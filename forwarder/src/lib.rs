#![cfg_attr(target_arch = "wasm32", no_std)]
#![allow(clippy::module_name_repetitions, clippy::as_conversions)]

use borsh::BorshDeserialize;
use core::alloc::{GlobalAlloc, Layout};

use crate::error::ContractError;
use crate::params::{
    ft_balance_args, ft_transfer_args, ft_transfer_call_args, FeesParams, FinishForwardParams,
    ForwardParams, State,
};
use crate::runtime::{panic_utf8, Env, PromiseHandler, Runtime, SdkExpect, SdkUnwrap, IO};
use crate::types::{
    AccountId, PromiseAction, PromiseBatchAction, PromiseCreateArgs, PromiseResult,
    PromiseWithCallbackArgs, Vec,
};

mod error;
mod params;
mod runtime;
mod types;

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATOR: NoopAllocator = NoopAllocator;

const MINIMUM_BALANCE: u128 = 360_000_000_000_000_000_000_000;
const ZERO_YOCTO: u128 = 0;
const MAX_FEE_PERCENT: u128 = 10;

const CALCULATE_FEES_GAS: u64 = 4_000_000_000_000;
const NEAR_DEPOSIT_GAS: u64 = 2_000_000_000_000;
const FT_BALANCE_GAS: u64 = 2_000_000_000_000;
const FT_TRANSFER_GAS: u64 = 3_000_000_000_000;
const FT_TRANSFER_CALL_GAS: u64 = 50_000_000_000_000;
const CALCULATE_FEES_CALLBACK_GAS: u64 = 90_000_000_000_000;
const FINISH_FORWARD_GAS: u64 = 70_000_000_000_000;

// Key is used for upgrading the smart contract.
// base58 representation of the key is: "ed25519:BaiF3VUJf5pxB9ezVtzH4SejpdYc7EA3SqrKczsj1wno";
#[cfg(not(feature = "tests"))]
const UPDATER_PK: [u8; 33] = [
    0, 157, 55, 171, 39, 212, 8, 14, 19, 58, 101, 78, 158, 202, 229, 222, 152, 23, 144, 112, 79,
    136, 229, 203, 142, 41, 95, 170, 31, 58, 47, 213, 152,
];
// base58 representation of the key is: "ed25519:BhnXcbxBgniLoG5LEnyeYHkJvzpuzy22eFuzssNCBtu3";
#[cfg(feature = "tests")]
const UPDATER_PK: [u8; 33] = [
    0, 159, 7, 148, 129, 146, 220, 189, 217, 236, 230, 111, 126, 201, 235, 59, 13, 109, 76, 138,
    133, 249, 235, 39, 194, 138, 171, 236, 30, 35, 237, 155, 214,
];
// In case we get near as a token id it means we need to transfer native NEAR tokens.
const NEAR: &str = "near";

#[no_mangle]
pub extern "C" fn new() {
    let mut io = Runtime;

    if State::load(&io).is_some() {
        panic_utf8(b"ERR_ALREADY_INITIALIZED");
    }

    let state: State = io.read_input_borsh().sdk_unwrap();
    state.save(&mut io);

    let current_account_id = io.current_account_id();
    let promise = PromiseBatchAction {
        target_account_id: current_account_id,
        actions: [PromiseAction::AddFullAccessKey {
            public_key: UPDATER_PK,
            nonce: 0,
        }],
    };

    let promise_id = unsafe { io.promise_create_batch(&promise) };
    io.promise_return(promise_id);
}

#[no_mangle]
pub extern "C" fn forward() {
    let io = Runtime;

    io.assert_one_yocto().sdk_unwrap();

    let params: ForwardParams = io.read_input_borsh().sdk_unwrap();

    if params.token_id.as_str() == NEAR {
        forward_native_token(io);
    } else {
        forward_nep141_token(io, params.token_id);
    }
}

#[no_mangle]
pub extern "C" fn calculate_fees_callback() {
    let mut io = Runtime;
    io.assert_private_call().sdk_unwrap();

    let params: ForwardParams = io.read_input_borsh().sdk_unwrap();
    let state = State::load(&io).sdk_expect("No state");
    let amount: u128 = match io.promise_result(0).sdk_expect("No promise result") {
        PromiseResult::Successful(v) => params::vec_to_number(&v).sdk_unwrap(),
        _ => panic_utf8(b"FEE RESULT IS NOT READY"),
    };

    let promise_id = unsafe {
        let promise_id = io.promise_create_and_combine(&[PromiseCreateArgs {
            target_account_id: state.fees_contract_id,
            method: "calculate_fees",
            args: types::to_borsh(&FeesParams {
                amount,
                token_id: &params.token_id,
                target_network: &state.target_network,
                target_address: state.target_address,
            })
            .sdk_unwrap(),
            attached_balance: ZERO_YOCTO,
            attached_gas: CALCULATE_FEES_GAS,
        }]);

        io.promise_attach_callback(
            promise_id,
            &PromiseCreateArgs {
                target_account_id: io.current_account_id(),
                method: "finish_forward_callback",
                args: types::to_borsh(&FinishForwardParams {
                    amount,
                    token_id: params.token_id,
                    promise_idx: 0,
                })
                .sdk_unwrap(),
                attached_balance: 2,
                attached_gas: FINISH_FORWARD_GAS,
            },
        )
    };

    io.promise_return(promise_id);
}

#[no_mangle]
pub extern "C" fn finish_forward_callback() {
    let mut io = Runtime;
    io.assert_private_call().sdk_unwrap();

    let params: FinishForwardParams = io.read_input_borsh().sdk_unwrap();
    let state = State::load(&io).sdk_expect("No state");
    let fee: u128 = match io
        .promise_result(params.promise_idx)
        .sdk_expect("No promise result")
    {
        PromiseResult::Successful(v) => u128::try_from_slice(&v)
            .map_err(|_| ContractError::BorshDeserializeError)
            .sdk_unwrap(),
        _ => panic_utf8(b"FEE RESULT IS NOT READY"),
    };

    if !is_fee_allowed(params.amount, fee) {
        panic_utf8(b"FEE IS TOO BIG");
    }

    let amount = params.amount.saturating_sub(fee);

    let mut promise_id = unsafe {
        io.promise_create_call(&PromiseCreateArgs {
            target_account_id: params.token_id,
            method: "ft_transfer_call",
            args: ft_transfer_call_args(&state.target_network, amount, state.target_address),
            attached_balance: 1,
            attached_gas: FT_TRANSFER_CALL_GAS,
        })
    };

    if fee > 0 {
        promise_id = unsafe {
            io.promise_attach_callback(
                promise_id,
                &PromiseCreateArgs {
                    target_account_id: params.token_id,
                    method: "ft_transfer",
                    args: ft_transfer_args(&state.fees_contract_id, fee),
                    attached_balance: 1,
                    attached_gas: FT_TRANSFER_GAS,
                },
            )
        };
    }

    io.promise_return(promise_id);
}

fn forward_native_token<I: IO + Env + PromiseHandler>(mut io: I) {
    let amount = io
        .account_balance()
        .checked_sub(MINIMUM_BALANCE)
        .filter(|a| *a > 0)
        .expect("Too low balance");

    let state = State::load(&io).unwrap();

    let promise_id = unsafe {
        let promise_id = io.promise_create_and_combine(&[
            PromiseCreateArgs {
                target_account_id: state.wnear_contract_id,
                method: "near_deposit",
                args: Vec::new(),
                attached_balance: amount,
                attached_gas: NEAR_DEPOSIT_GAS,
            },
            PromiseCreateArgs {
                target_account_id: state.fees_contract_id,
                method: "calculate_fees",
                args: types::to_borsh(&FeesParams {
                    amount,
                    token_id: &state.wnear_contract_id,
                    target_network: &state.target_network,
                    target_address: state.target_address,
                })
                .sdk_unwrap(),
                attached_balance: ZERO_YOCTO,
                attached_gas: CALCULATE_FEES_GAS,
            },
        ]);

        io.promise_attach_callback(
            promise_id,
            &PromiseCreateArgs {
                target_account_id: io.current_account_id(),
                method: "finish_forward_callback",
                args: types::to_borsh(&FinishForwardParams {
                    amount,
                    token_id: state.wnear_contract_id,
                    promise_idx: 1,
                })
                .sdk_unwrap(),
                attached_balance: 2,
                attached_gas: FINISH_FORWARD_GAS,
            },
        )
    };

    io.promise_return(promise_id);
}

fn forward_nep141_token<I: IO + Env + PromiseHandler>(mut io: I, token_id: AccountId) {
    let callback_args = types::to_borsh(&token_id).sdk_unwrap();
    let promise_id = unsafe {
        io.promise_create_with_callback(&PromiseWithCallbackArgs {
            base: PromiseCreateArgs {
                target_account_id: token_id,
                method: "ft_balance_of",
                args: ft_balance_args(&io.current_account_id()),
                attached_balance: ZERO_YOCTO,
                attached_gas: FT_BALANCE_GAS,
            },
            callback: PromiseCreateArgs {
                target_account_id: io.current_account_id(),
                method: "calculate_fees_callback",
                args: callback_args,
                attached_balance: ZERO_YOCTO,
                attached_gas: CALCULATE_FEES_CALLBACK_GAS,
            },
        })
    };

    io.promise_return(promise_id);
}

// Validate that calculated part of the fee isn't more than `MAX_FEE_PERCENT`.
fn is_fee_allowed(amount: u128, fee: u128) -> bool {
    match (fee * 100)
        .checked_div(amount)
        .zip((fee * 100).checked_rem(amount))
    {
        Some((percent, _)) if percent > MAX_FEE_PERCENT => false,
        Some((percent, reminder)) if percent == MAX_FEE_PERCENT && reminder > 0 => false,
        _ => true,
    }
}

struct NoopAllocator;

unsafe impl GlobalAlloc for NoopAllocator {
    unsafe fn alloc(&self, _: Layout) -> *mut u8 {
        core::ptr::null_mut()
    }

    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}

#[cfg(target_arch = "wasm32")]
#[panic_handler]
/// On panic handler.
/// # Safety
pub unsafe fn on_panic(_: &::core::panic::PanicInfo) -> ! {
    ::core::arch::wasm32::unreachable();
}
