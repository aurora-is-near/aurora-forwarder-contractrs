use aurora_engine_types::types::Address;
use forwarder_utils::forwarder_prefix;
use near_sdk::serde_json::json;
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::types::NearToken;
use near_workspaces::{Account, AccountId, Contract, InMemorySigner, Worker};

pub mod aurora;
pub mod erc20;
pub mod factory;
pub mod forwarder;
pub mod fungible_token;

const AURORA_WASM_PATH: &str = "../res/aurora-mainnet.wasm";
const FT_WASM_PATH: &str = "../res/fungible-token.wasm";
const FORWARDER_WASM_PATH: &str = "../res/aurora-forwarder.wasm";
const FORWARDER_TESTS_WASM_PATH: &str = "../res/aurora-forwarder-tests.wasm";
const FEES_WASM_PATH: &str = "../res/aurora-forwarder-fees.wasm";
const FACTORY_WASM_PATH: &str = "../res/aurora-forwarder-factory-tests.wasm";
const WRAP_WASM_PATH: &str = "../res/w_near.wasm";
const INIT_BALANCE_NEAR: NearToken = NearToken::from_near(50);
const FORWARDER_MIN_BALANCE: NearToken = NearToken::from_near(2);

pub struct Sandbox {
    worker: Worker<near_workspaces::network::Sandbox>,
    root_account: Account,
}

impl Sandbox {
    pub async fn new() -> anyhow::Result<Self> {
        let worker = near_workspaces::sandbox().await?;
        let root_account = worker.root_account()?;

        Ok(Self {
            worker,
            root_account,
        })
    }

    pub async fn create_subaccount(
        &self,
        name: &str,
        balance: NearToken,
    ) -> anyhow::Result<Account> {
        self.root_account
            .create_subaccount(name)
            .initial_balance(balance)
            .transact()
            .await
            .map(|result| result.result)
            .map_err(Into::into)
    }

    pub async fn balance(&self, account_id: &AccountId) -> u128 {
        self.worker
            .view_account(account_id)
            .await
            .unwrap()
            .balance
            .as_yoctonear()
    }

    pub async fn delete_account(
        &self,
        account_id: &AccountId,
        signer: &InMemorySigner,
        beneficiary_id: &AccountId,
    ) -> anyhow::Result<ExecutionFinalResult> {
        self.worker
            .delete_account(account_id, signer, beneficiary_id)
            .await
            .map_err(Into::into)
    }

    pub async fn deploy_ft(
        &self,
        total_supply: u128,
        name: &str,
        decimals: u8,
    ) -> anyhow::Result<(Contract, Account)> {
        let name_lower = name.to_lowercase();
        let owner_name = format!("{}-owner", &name_lower);
        let ft_owner_account = self
            .create_subaccount(&owner_name, INIT_BALANCE_NEAR)
            .await?;
        let ft_contract_account = self
            .create_subaccount(&name_lower, INIT_BALANCE_NEAR)
            .await?;
        let result = ft_contract_account.deploy(&code(FT_WASM_PATH)).await?;
        assert!(result.is_success());

        let contract = result.result;
        let result = contract
            .call("new")
            .args_json(json!({
                "owner_id": ft_owner_account.id(),
                "total_supply": total_supply.to_string(),
                "metadata": {
                    "spec": "ft-1.0.0",
                    "name": format!("Token {}", &name),
                    "symbol": name,
                    "decimals": decimals
                }
            }))
            .max_gas()
            .transact()
            .await?;
        assert!(result.is_success(), "{result:?}");

        Ok((contract, ft_owner_account))
    }

    pub async fn deploy_wrap_near(&self) -> anyhow::Result<(Contract, Account)> {
        let wrap_owner_account = self
            .create_subaccount("wrap-owner", INIT_BALANCE_NEAR)
            .await?;
        let wrap_account = self.create_subaccount("wrap", INIT_BALANCE_NEAR).await?;
        let result = wrap_account.deploy(&code(WRAP_WASM_PATH)).await?;
        assert!(result.is_success());

        let contract = result.result;
        let result = contract.call("new").max_gas().transact().await?;
        assert!(result.is_success(), "{result:?}");

        let result = wrap_owner_account
            .call(contract.id(), "near_deposit")
            .deposit(NearToken::from_near(20))
            .transact()
            .await
            .unwrap();
        assert!(result.is_success(), "{result:?}");

        Ok((contract, wrap_owner_account))
    }

    pub async fn deploy_aurora(&self, name: &str) -> anyhow::Result<Contract> {
        let aurora_account = self.create_subaccount(name, INIT_BALANCE_NEAR).await?;
        let result = aurora_account.deploy(&code(AURORA_WASM_PATH)).await?;
        assert!(result.is_success());
        let contract = result.result;
        let result = aurora_account
            .call(contract.id(), "new")
            .args_json(json!({
               "chain_id": 1_313_161_559,
                "owner_id": self.root_account.id(),
                "upgrade_delay_blocks": 0,
                "key_manager": self.root_account.id(),
                "initial_hashchain": null
            }))
            .max_gas()
            .transact()
            .await?;
        assert!(result.is_success());

        Ok(contract)
    }

    pub async fn deploy_forwarder(
        &self,
        target_network: &AccountId,
        address: &str,
        fees_account_id: &AccountId,
        wnear_contract_id: &AccountId,
    ) -> anyhow::Result<Contract> {
        self.deploy_forwarder_with_wasm(
            target_network,
            address,
            fees_account_id,
            wnear_contract_id,
            FORWARDER_TESTS_WASM_PATH,
        )
        .await
    }

    pub async fn deploy_prod_forwarder(
        &self,
        target_network: &AccountId,
        address: &str,
        fees_account_id: &AccountId,
        wnear_contract_id: &AccountId,
    ) -> anyhow::Result<Contract> {
        self.deploy_forwarder_with_wasm(
            target_network,
            address,
            fees_account_id,
            wnear_contract_id,
            FORWARDER_WASM_PATH,
        )
        .await
    }

    async fn deploy_forwarder_with_wasm(
        &self,
        target_network: &AccountId,
        address: &str,
        fees_account_id: &AccountId,
        wnear_contract_id: &AccountId,
        wasm: &str,
    ) -> anyhow::Result<Contract> {
        let name = forwarder_prefix(
            address,
            &target_network.as_str().parse().unwrap(),
            &fees_account_id.as_str().parse().unwrap(),
        );
        let fwd_account = self.create_subaccount(&name, FORWARDER_MIN_BALANCE).await?;
        let result = fwd_account.deploy(&code(wasm)).await?;
        assert!(result.is_success());
        let contract = result.result;
        let address = Address::decode(address.trim_start_matches("0x")).unwrap();
        let result = fwd_account
            .call(contract.id(), "new")
            .args_borsh((address, target_network, wnear_contract_id, fees_account_id))
            .max_gas()
            .transact()
            .await?;
        assert!(result.is_success());

        Ok(contract)
    }

    pub async fn deploy_fees(&self, supported_tokens: &[&AccountId]) -> anyhow::Result<Contract> {
        let fee_account = self.create_subaccount("fees", INIT_BALANCE_NEAR).await?;
        let result = fee_account.deploy(&code(FEES_WASM_PATH)).await?;
        assert!(result.is_success());
        let contract = result.result;
        let result = fee_account
            .call(contract.id(), "new")
            .args_json(json!({
                "tokens": supported_tokens
            }))
            .max_gas()
            .transact()
            .await?;
        assert!(result.is_success());

        Ok(contract)
    }

    pub async fn deploy_factory(&self, fees_contract_id: &AccountId) -> anyhow::Result<Contract> {
        let factory_account = self.create_subaccount("factory", INIT_BALANCE_NEAR).await?;
        let result = factory_account.deploy(&code(FACTORY_WASM_PATH)).await?;
        assert!(result.is_success());
        let contract = result.result;
        let result = factory_account
            .call(contract.id(), "new")
            .args_json(json!({
                "fees_contract_id": fees_contract_id
            }))
            .max_gas()
            .transact()
            .await?;
        assert!(result.is_success());

        Ok(contract)
    }
}

fn code(path: &str) -> Vec<u8> {
    std::fs::read(path)
        .unwrap_or_else(|e| panic!("couldn't get WASM code for with path: {path}, error: {e}"))
}
