use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::json_types::{U128, U64};
use near_sdk::{env, near_bindgen, AccountId, IntoStorageKey, PanicOnDefault};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::num::ParseFloatError;
use std::str::FromStr;

// We multiply percents to 100 here to get rid of the floating numbers.
const MIN_FEE_PERCENT: u64 = 1; // 0.01 %
const MAX_FEE_PERCENT: u64 = 1000; // 10 %
const DEFAULT_PERCENT: U64 = U64(500); // 5%

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct FeesCalculator {
    percent: U64,
    owner: AccountId,
    supported_tokens: BTreeSet<AccountId>,
}

#[near_bindgen]
impl FeesCalculator {
    /// Contract's constructor.
    ///
    /// # Panics
    ///
    /// The constructor panics if the state already exists.
    #[init]
    #[must_use]
    pub fn new(tokens: Vec<AccountId>) -> Self {
        Self {
            percent: DEFAULT_PERCENT,
            owner: env::predecessor_account_id(),
            supported_tokens: tokens.into_iter().collect(),
        }
    }

    /// Calculate and return the fee for the corresponding token and Aurora Network.
    #[must_use]
    pub fn calculate_fees(
        &self,
        amount: U128,
        token_id: &AccountId,
        target_network: &AccountId,
        target_address: String,
    ) -> U128 {
        let _ = (target_network, target_address);

        if self.supported_tokens.contains(token_id) {
            u128::from(self.percent.0)
                .checked_mul(amount.0)
                .unwrap_or_default()
                .saturating_div(10000)
                .into()
        } else {
            0.into()
        }
    }

    /// Set the percent of the fee.
    ///
    /// # Panics
    ///
    /// Panics if the invoker of the transaction is not owner.
    #[allow(clippy::needless_pass_by_value)]
    pub fn set_fee_percent(&mut self, percent: String) {
        assert_eq!(env::predecessor_account_id(), self.owner);

        match parse_percent(&percent) {
            Ok(value) => self.percent = value,
            Err(e) => env::panic_str(&format!("Couldn't parse percent: {e}")),
        }
    }

    /// Returns current fee percent.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn get_fee_percent(&self) -> String {
        format!("{:.2}", self.percent.0 as f64 / 100.0)
    }

    /// Return a list of supported tokens.
    #[must_use]
    pub fn supported_tokens(&self) -> Vec<&AccountId> {
        self.supported_tokens.iter().collect()
    }

    /// Add an account id of a new supported NEP-141 token.
    ///
    /// # Panics
    ///
    /// Panic if the added token is already exist.
    pub fn add_supported_token(&mut self, token_id: AccountId) {
        assert!(
            self.supported_tokens.insert(token_id),
            "Token is already present"
        );
    }

    /// Remove the token from the list of supported.
    ///
    /// # Panics
    ///
    /// Panics if the removed token is not exists.
    pub fn remove_supported_token(&mut self, token_id: &AccountId) {
        assert!(
            self.supported_tokens.remove(token_id),
            "Nothing to remove, token: {token_id} hasn't been added"
        );
    }
}

#[derive(BorshDeserialize, BorshSerialize)]
enum KeyPrefix {
    SupportedTokens,
}

impl IntoStorageKey for KeyPrefix {
    fn into_storage_key(self) -> Vec<u8> {
        match self {
            Self::SupportedTokens => b"supported_tokens".to_vec(),
        }
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_percent(percent: &str) -> Result<U64, ParseError> {
    validate_decimal_part(percent)?;

    let result = f64::from_str(percent)
        .map(|p| (p * 100.0) as u64) // as conversion is safe here because we validate the number of decimals
        .map_err(ParseError::ParseFloat)?;

    if result < MIN_FEE_PERCENT {
        Err(ParseError::TooLowPercent)
    } else if result > MAX_FEE_PERCENT {
        Err(ParseError::TooHighPercent)
    } else {
        Ok(U64(result))
    }
}

#[derive(Debug)]
enum ParseError {
    ParseFloat(ParseFloatError),
    TooLowPercent,
    TooHighPercent,
    TooManyDecimals,
}

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        #[allow(deprecated)]
        let msg = match self {
            Self::ParseFloat(error) => error.description(),
            Self::TooLowPercent => "provided percent is less than 0.01%",
            Self::TooHighPercent => "provided percent is more than 10%",
            Self::TooManyDecimals => "provided percent could contain only 2 decimals",
        };

        f.write_str(msg)
    }
}

fn validate_decimal_part(percent: &str) -> Result<(), ParseError> {
    match percent.split_once('.') {
        Some((_, decimal)) if decimal.len() > 2 => Err(ParseError::TooManyDecimals),
        _ => Ok(()), // no decimals or the number of decimals is less or equal 2.
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_percent, FeesCalculator, ParseError};

    #[test]
    fn test_parse_percent() {
        assert_eq!(parse_percent("10").unwrap(), 1000.into());
        assert_eq!(parse_percent("2").unwrap(), 200.into());
        assert_eq!(parse_percent("0.25").unwrap(), 25.into());
        assert_eq!(parse_percent("0.01").unwrap(), 1.into());
        assert!(matches!(
            parse_percent("0.015").err(),
            Some(ParseError::TooManyDecimals)
        ));
        assert!(matches!(
            parse_percent("0.009").err(),
            Some(ParseError::TooManyDecimals)
        ));
        assert!(matches!(
            parse_percent("10.1").err(),
            Some(ParseError::TooHighPercent)
        ));
        assert!(matches!(
            parse_percent("hello").err(),
            Some(ParseError::ParseFloat(_))
        ));
    }

    #[test]
    fn test_check_supported_tokens() {
        let aurora = "aurora".parse().unwrap();
        let target_address = "0xea2342".to_string();
        let usdt = "usdt.near".parse().unwrap();
        let mut contract = FeesCalculator::new(vec![]);

        assert_eq!(
            contract.calculate_fees(1000.into(), &usdt, &aurora, target_address.clone()),
            0.into() // we don't support the `usdt.near` yet, so we get 0 here
        );

        contract.add_supported_token(usdt.clone());

        assert_eq!(
            contract.calculate_fees(1000.into(), &usdt, &aurora, target_address.clone()),
            50.into()
        );

        contract.remove_supported_token(&usdt);

        assert_eq!(
            contract.calculate_fees(1000.into(), &usdt, &aurora, target_address),
            0.into()
        );
    }

    #[test]
    fn test_set_percent() {
        let mut contract = FeesCalculator::new(vec![]);

        assert_eq!(contract.get_fee_percent(), "5.00");
        contract.set_fee_percent("6".to_string());
        assert_eq!(contract.get_fee_percent(), "6.00");
        contract.set_fee_percent("7.5".to_string());
        assert_eq!(contract.get_fee_percent(), "7.50");
    }

    #[test]
    #[should_panic(
        expected = "Couldn't parse percent: provided percent could contain only 2 decimals"
    )]
    fn test_set_percent_with_many_decimals() {
        let mut contract = FeesCalculator::new(vec![]);
        contract.set_fee_percent("6.123".to_string());
    }

    #[test]
    #[should_panic(expected = "Couldn't parse percent: provided percent is more than 10%")]
    fn test_set_too_high_percents() {
        let mut contract = FeesCalculator::new(vec![]);
        contract.set_fee_percent("12.12".to_string());
    }
}
