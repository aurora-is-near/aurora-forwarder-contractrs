use crate::sandbox::{aurora::Aurora, fungible_token::FungibleToken, Sandbox};
use aurora_engine_types::types::Address;
use aurora_forwarder_factory::{DeployParameters, INIT_BALANCE, MAX_NUM_CONTRACTS};
use near_workspaces::types::{AccessKeyPermission, NearToken, PublicKey, SecretKey};
use near_workspaces::{AccountId, InMemorySigner};
use once_cell::sync::Lazy;
use std::str::FromStr;

mod native;
mod wrap;

const RECEIVER: &str = "0x17ffdf6becbbc34d5c7d3bf4a0ed4a680395d057";
const TOTAL_SUPPLY: u128 = 1_000_000_000_000_000;

static WNEAR: Lazy<AccountId> = Lazy::new(|| AccountId::from_str("wrap.test.near").unwrap());

#[tokio::test]
async fn test_creating_ft() {
    let sandbox = Sandbox::new().await.unwrap();
    let (ft, ft_owner) = sandbox.deploy_ft(TOTAL_SUPPLY, "USDT", 6).await.unwrap();
    let owner_balance = ft.ft_balance_of(ft_owner.id()).await;
    assert_eq!(owner_balance, TOTAL_SUPPLY);
    let balance = NearToken::from_near(10);
    let alice = sandbox.create_subaccount("alice", balance).await.unwrap();
    assert_eq!(
        sandbox.balance(alice.id()).await,
        NearToken::from_near(10).as_yoctonear()
    );
    ft.storage_deposit(alice.id()).await.unwrap();

    let alice_balance = ft.ft_balance_of(alice.id()).await;
    assert_eq!(alice_balance, 0);

    let transfer_result = ft.ft_transfer(&ft_owner, alice.id(), 50).await;
    assert!(transfer_result.is_ok());

    let alice_balance = ft.ft_balance_of(alice.id()).await;
    assert_eq!(alice_balance, 50);
}

#[tokio::test]
async fn test_creating_forwarder() {
    let sandbox = Sandbox::new().await.unwrap();
    let aurora = sandbox.deploy_aurora("aurora").await.unwrap();
    let fees = sandbox.deploy_fees(&[]).await.unwrap();
    let result = sandbox
        .deploy_forwarder(
            aurora.id(),
            "0x17ffdf6becbbc34d5c7d3bf4a0ed4a680395d057",
            fees.id(),
            &WNEAR,
        )
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_creating_erc20() {
    let sandbox = Sandbox::new().await.unwrap();
    let (ft, _) = sandbox.deploy_ft(TOTAL_SUPPLY, "USDC", 6).await.unwrap();
    let aurora = sandbox.deploy_aurora("aurora").await.unwrap();
    let erc20 = aurora.deploy_erc20(ft.id()).await.unwrap();
    assert_eq!(
        erc20.address(),
        "0x35c61bd8f7cb50410abded58646dbdd6c447d135"
    );
}

#[tokio::test]
async fn test_main_successful_flow() {
    use crate::sandbox::forwarder::Forwarder;

    let forward_amount = 1_000_000_000;
    let fee_percent = 5;
    let sandbox = Sandbox::new().await.unwrap();
    let (ft, ft_owner) = sandbox.deploy_ft(TOTAL_SUPPLY, "USDT", 6).await.unwrap();

    let aurora = sandbox.deploy_aurora("aurora").await.unwrap();
    ft.storage_deposit(aurora.id()).await.unwrap();

    let erc20 = aurora.deploy_erc20(ft.id()).await.unwrap();
    assert_eq!(erc20.balance_of(RECEIVER).await, 0);

    let fees = sandbox.deploy_fees(&[ft.id()]).await.unwrap();
    ft.storage_deposit(fees.id()).await.unwrap();

    let forwarder = sandbox
        .deploy_forwarder(aurora.id(), RECEIVER, fees.id(), &WNEAR)
        .await
        .unwrap();
    ft.storage_deposit(forwarder.id()).await.unwrap();

    ft.ft_transfer(&ft_owner, forwarder.id(), forward_amount)
        .await
        .unwrap();

    assert_eq!(ft.ft_balance_of(forwarder.id()).await, forward_amount);
    assert_eq!(ft.ft_balance_of(aurora.id()).await, 0);

    forwarder.forward(ft.id()).await.unwrap();

    let fee = (forward_amount * fee_percent) / 100;
    let balance = forward_amount - fee;

    assert_eq!(erc20.balance_of(RECEIVER).await, balance);
    assert_eq!(ft.ft_balance_of(aurora.id()).await, balance);
    assert_eq!(ft.ft_balance_of(fees.id()).await, fee);
    assert_eq!(ft.ft_balance_of(forwarder.id()).await, 0);
    assert_eq!(
        ft.ft_balance_of(ft_owner.id()).await,
        TOTAL_SUPPLY - forward_amount
    );
}

#[allow(clippy::similar_names)]
#[tokio::test]
async fn test_forward_two_tokens() {
    use crate::sandbox::factory::Factory;

    let forward_amount = 1_000_000_000;
    let fee_percent = 5;
    let sandbox = Sandbox::new().await.unwrap();
    let (usdt, usdt_owner) = sandbox.deploy_ft(TOTAL_SUPPLY, "USDT", 6).await.unwrap();
    let (usdc, usdc_owner) = sandbox.deploy_ft(TOTAL_SUPPLY, "USDC", 6).await.unwrap();

    let aurora = sandbox.deploy_aurora("aurora").await.unwrap();
    usdt.storage_deposit(aurora.id()).await.unwrap();
    usdc.storage_deposit(aurora.id()).await.unwrap();

    let usdt_erc20 = aurora.deploy_erc20(usdt.id()).await.unwrap();
    let usdc_erc20 = aurora.deploy_erc20(usdc.id()).await.unwrap();
    assert_eq!(usdt_erc20.balance_of(RECEIVER).await, 0);
    assert_eq!(usdc_erc20.balance_of(RECEIVER).await, 0);

    let fees = sandbox.deploy_fees(&[usdt.id(), usdc.id()]).await.unwrap();
    usdt.storage_deposit(fees.id()).await.unwrap();
    usdc.storage_deposit(fees.id()).await.unwrap();

    let factory = sandbox.deploy_factory(fees.id()).await.unwrap();
    let mut ids = factory
        .create(&[DeployParameters {
            target_address: RECEIVER.to_string(),
            target_network: aurora.id().as_str().parse().unwrap(),
            wnear_contract_id: WNEAR.clone(),
        }])
        .await
        .unwrap();
    let forwarder_id = ids.pop().unwrap();

    usdt.storage_deposit(&forwarder_id).await.unwrap();
    usdc.storage_deposit(&forwarder_id).await.unwrap();

    usdt.ft_transfer(&usdt_owner, &forwarder_id, forward_amount)
        .await
        .unwrap();
    usdc.ft_transfer(&usdc_owner, &forwarder_id, forward_amount)
        .await
        .unwrap();

    assert_eq!(usdt.ft_balance_of(&forwarder_id).await, forward_amount);
    assert_eq!(usdc.ft_balance_of(&forwarder_id).await, forward_amount);
    assert_eq!(usdt.ft_balance_of(aurora.id()).await, 0);
    assert_eq!(usdc.ft_balance_of(aurora.id()).await, 0);

    factory.forward(&forwarder_id, usdt.id()).await.unwrap();
    factory.forward(&forwarder_id, usdc.id()).await.unwrap();

    let fee = (forward_amount * fee_percent) / 100;
    let balance = forward_amount - fee;

    // Check USDT
    assert_eq!(usdt_erc20.balance_of(RECEIVER).await, balance);
    assert_eq!(usdt.ft_balance_of(aurora.id()).await, balance);
    assert_eq!(usdt.ft_balance_of(fees.id()).await, fee);
    assert_eq!(usdt.ft_balance_of(&forwarder_id).await, 0);
    assert_eq!(
        usdt.ft_balance_of(usdt_owner.id()).await,
        TOTAL_SUPPLY - forward_amount
    );
    // Check USDC
    assert_eq!(usdc_erc20.balance_of(RECEIVER).await, balance);
    assert_eq!(usdc.ft_balance_of(aurora.id()).await, balance);
    assert_eq!(usdc.ft_balance_of(fees.id()).await, fee);
    assert_eq!(usdc.ft_balance_of(&forwarder_id).await, 0);
    assert_eq!(
        usdc.ft_balance_of(usdc_owner.id()).await,
        TOTAL_SUPPLY - forward_amount
    );
}

#[tokio::test]
async fn test_using_full_access_key() {
    let sandbox = Sandbox::new().await.unwrap();
    let pk = PublicKey::from_str("ed25519:BhnXcbxBgniLoG5LEnyeYHkJvzpuzy22eFuzssNCBtu3").unwrap();
    let silo_account_id = "some.silo.near".parse().unwrap();
    let fees_account_id = "fees.near".parse().unwrap();
    let forwarder = sandbox
        .deploy_forwarder(&silo_account_id, RECEIVER, &fees_account_id, &WNEAR)
        .await
        .unwrap();
    let key = forwarder.view_access_key(&pk).await.unwrap();
    assert!(matches!(key.permission, AccessKeyPermission::FullAccess));
}

#[tokio::test]
async fn test_using_prod_full_access_key() {
    let sandbox = Sandbox::new().await.unwrap();
    let pk = PublicKey::from_str("ed25519:BaiF3VUJf5pxB9ezVtzH4SejpdYc7EA3SqrKczsj1wno").unwrap();
    let silo_account_id = "some.silo.near".parse().unwrap();
    let fees_account_id = "fees.near".parse().unwrap();
    let forwarder = sandbox
        .deploy_prod_forwarder(&silo_account_id, RECEIVER, &fees_account_id, &WNEAR)
        .await
        .unwrap();
    let key = forwarder.view_access_key(&pk).await.unwrap();
    assert!(matches!(key.permission, AccessKeyPermission::FullAccess));
}

#[tokio::test]
async fn test_using_factory() {
    use crate::sandbox::factory::Factory;

    let sandbox = Sandbox::new().await.unwrap();
    let fees = sandbox.deploy_fees(&[]).await.unwrap();
    let _ = sandbox.deploy_wrap_near().await.unwrap();
    let factory = sandbox.deploy_factory(fees.id()).await.unwrap();
    let parameters = (0..MAX_NUM_CONTRACTS)
        .map(|i| DeployParameters {
            target_address: Address::from_array([u8::try_from(i).unwrap_or_default(); 20]).encode(),
            target_network: format!("silo-{i}.test.near").parse().unwrap(),
            wnear_contract_id: WNEAR.as_str().parse().unwrap(),
        })
        .collect::<Vec<_>>();
    let forwarder_ids = factory.create(&parameters).await.unwrap();
    let factory_id = factory.id();
    let fees_id = fees.id();

    assert_eq!(forwarder_ids.len(), MAX_NUM_CONTRACTS);

    for (id, params) in forwarder_ids.iter().zip(parameters) {
        assert!(sandbox.balance(id).await > INIT_BALANCE.as_yoctonear());

        let expected_id = format!(
            "{}.{factory_id}",
            forwarder_utils::forwarder_prefix(
                &params.target_address,
                &params.target_network,
                &fees_id.as_str().parse().unwrap(),
            )
        );

        assert_eq!(id.as_str(), expected_id);
    }
}

#[tokio::test]
#[allow(clippy::similar_names)]
async fn test_successful_complicated_flow() {
    use crate::sandbox::factory::Factory;
    let sandbox = Sandbox::new().await.unwrap();

    let alice_address = "0x41e60a647bc61097ed52f15855fcf24a9dacdbe4";
    let bob_address = "0xbf85c24ca42c553dbabc3ec944142e290fc3c82b";
    let john_address = "0x78aea1a9f9880b0cda3bc0b96251948be4f340ca";

    let (usdt, usdt_owner) = sandbox.deploy_ft(TOTAL_SUPPLY, "USDT", 6).await.unwrap();
    let (usdc, usdc_owner) = sandbox.deploy_ft(TOTAL_SUPPLY, "USDC", 6).await.unwrap();

    let silo1 = sandbox.deploy_aurora("silo-1").await.unwrap();
    let silo2 = sandbox.deploy_aurora("silo-2").await.unwrap();
    let silo3 = sandbox.deploy_aurora("silo-3").await.unwrap();

    usdt.storage_deposit(silo1.id()).await.unwrap();
    usdc.storage_deposit(silo1.id()).await.unwrap();
    usdt.storage_deposit(silo2.id()).await.unwrap();
    usdc.storage_deposit(silo2.id()).await.unwrap();
    usdt.storage_deposit(silo3.id()).await.unwrap();
    usdc.storage_deposit(silo3.id()).await.unwrap();

    let usdt_erc20_s1 = silo1.deploy_erc20(usdt.id()).await.unwrap();
    let usdc_erc20_s1 = silo1.deploy_erc20(usdc.id()).await.unwrap();
    let usdt_erc20_s2 = silo2.deploy_erc20(usdt.id()).await.unwrap();
    let usdc_erc20_s2 = silo2.deploy_erc20(usdc.id()).await.unwrap();
    let usdt_erc20_s3 = silo3.deploy_erc20(usdt.id()).await.unwrap();
    let usdc_erc20_s3 = silo3.deploy_erc20(usdc.id()).await.unwrap();

    let fees = sandbox.deploy_fees(&[usdt.id(), usdc.id()]).await.unwrap();

    usdt.storage_deposit(fees.id()).await.unwrap();
    usdc.storage_deposit(fees.id()).await.unwrap();

    let factory = sandbox.deploy_factory(fees.id()).await.unwrap();

    let parameters = [
        DeployParameters {
            target_address: alice_address.to_string(),
            target_network: silo1.id().as_str().parse().unwrap(),
            wnear_contract_id: WNEAR.clone(),
        },
        DeployParameters {
            target_address: bob_address.to_string(),
            target_network: silo2.id().as_str().parse().unwrap(),
            wnear_contract_id: WNEAR.clone(),
        },
        DeployParameters {
            target_address: john_address.to_string(),
            target_network: silo3.id().as_str().parse().unwrap(),
            wnear_contract_id: WNEAR.clone(),
        },
    ];
    let forward_ids: [_; 3] = factory
        .create(&parameters)
        .await
        .unwrap()
        .try_into()
        .unwrap();

    for fwd_id in &forward_ids {
        usdt.storage_deposit(fwd_id).await.unwrap();
        usdc.storage_deposit(fwd_id).await.unwrap();
    }

    let [alice_fwd, bob_fwd, john_fwd] = &forward_ids;

    usdt.ft_transfer(&usdt_owner, alice_fwd, 8_000_000)
        .await
        .unwrap();
    usdt.ft_transfer(&usdt_owner, bob_fwd, 6_000_000)
        .await
        .unwrap();
    usdt.ft_transfer(&usdt_owner, john_fwd, 4_000_000)
        .await
        .unwrap();
    usdc.ft_transfer(&usdc_owner, alice_fwd, 4_000_000)
        .await
        .unwrap();
    usdc.ft_transfer(&usdc_owner, bob_fwd, 6_000_000)
        .await
        .unwrap();
    usdc.ft_transfer(&usdc_owner, john_fwd, 8_000_000)
        .await
        .unwrap();

    for fwd_id in &forward_ids {
        factory.forward(fwd_id, usdt.id()).await.unwrap();
        factory.forward(fwd_id, usdc.id()).await.unwrap();
    }

    assert_eq!(usdt_erc20_s1.balance_of(alice_address).await, 7_600_000);
    assert_eq!(usdt_erc20_s2.balance_of(bob_address).await, 5_700_000);
    assert_eq!(usdt_erc20_s3.balance_of(john_address).await, 3_800_000);

    assert_eq!(usdc_erc20_s1.balance_of(alice_address).await, 3_800_000);
    assert_eq!(usdc_erc20_s2.balance_of(bob_address).await, 5_700_000);
    assert_eq!(usdc_erc20_s3.balance_of(john_address).await, 7_600_000);

    assert_eq!(usdt.ft_balance_of(fees.id()).await, 900_000);
    assert_eq!(usdc.ft_balance_of(fees.id()).await, 900_000);

    assert_eq!(usdt.ft_balance_of(silo1.id()).await, 7_600_000);
    assert_eq!(usdt.ft_balance_of(silo2.id()).await, 5_700_000);
    assert_eq!(usdt.ft_balance_of(silo3.id()).await, 3_800_000);

    assert_eq!(usdc.ft_balance_of(silo1.id()).await, 3_800_000);
    assert_eq!(usdc.ft_balance_of(silo2.id()).await, 5_700_000);
    assert_eq!(usdc.ft_balance_of(silo3.id()).await, 7_600_000);
}

// The goal of the test is to demonstrate that the factory can recover its storage staking after
// deleting the forwarder account (less the $NEAR spent to perform the delete action).
#[tokio::test]
#[allow(clippy::float_cmp)]
async fn test_storage_deposit_refund() {
    use crate::sandbox::factory::Factory;

    let sandbox = Sandbox::new().await.unwrap();
    let fees = sandbox.deploy_fees(&[]).await.unwrap();
    let (wnear, _) = sandbox.deploy_wrap_near().await.unwrap();
    let factory = sandbox.deploy_factory(fees.id()).await.unwrap();
    let balance_before_create = sandbox.balance(factory.id()).await;
    let parameters = DeployParameters {
        target_address: Address::from_array([1; 20]).encode(),
        target_network: "silo.test.near".parse().unwrap(),
        wnear_contract_id: wnear.id().as_str().parse().unwrap(),
    };

    let forwarder_ids = factory.create(&[parameters]).await.unwrap();
    assert_eq!(forwarder_ids.len(), 1);

    let balance_after_create = sandbox.balance(factory.id()).await;
    assert_eq!(
        to_near(balance_before_create - balance_after_create),
        0.363_685 // Ⓝ
    );

    let sk = SecretKey::from_str("ed25519:61TF7S52FVETjLp6KMUDp1TYBEdc1km1GnHgZc67VhWfyHTCUTMjUY6mM3qML17EAHFiutjpmF4CD9wdSGtG19tR").unwrap();
    let signer = InMemorySigner::from_secret_key(forwarder_ids[0].clone(), sk);
    let result = sandbox
        .delete_account(&forwarder_ids[0], &signer, factory.id())
        .await
        .unwrap();
    assert!(result.is_success());

    let balance_after_delete = sandbox.balance(factory.id()).await;
    println!("balance_after_delete: {balance_after_delete}");
    assert_eq!(
        to_near(balance_before_create - balance_after_delete),
        0.003_722 // Ⓝ
    );
}

fn to_near(amount: u128) -> f64 {
    u32::try_from(amount / 10_u128.pow(18))
        .map(f64::from)
        .map(|v| v / 1_000_000.0)
        .unwrap_or_default()
}
