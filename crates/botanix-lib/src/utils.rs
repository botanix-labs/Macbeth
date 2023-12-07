
use ethabi::ethereum_types::U256;

/// One satoshi expressed in wei.
///
/// This equals 10^10.
const SATOSHI_IN_WEI: U256 = U256([10_000_000_000, 0, 0, 0]);

/// The maximum [bitcoin::Amount] satoshi value.
///
/// This equals u64::max_value().
const MAX_SATOSHI: U256 = U256([u64::MAX, 0, 0, 0]);

/// An extension trait for [bitcoin::Amount].
pub trait AmountExt: Copy + From<bitcoin::Amount> + Into<bitcoin::Amount> {
    /// Convert this amount to the representation in wei.
    fn to_wei(self) -> U256 {
        U256::from(self.into().to_sat()) * SATOSHI_IN_WEI
    }

    /// Convert the amount represented in wei into an [bitcoin::Amount], by
    /// dropping the value that is smaller than one satoshi (rounding down).
    ///
    /// Returns [None] if the wei amount exceeds the maximum value of
    /// [bitcoin::Amount::max_value()].
    fn from_wei_floor(wei: U256) -> Option<Self> {
        let sat = wei / SATOSHI_IN_WEI;
        if sat <= MAX_SATOSHI {
            Some(bitcoin::Amount::from_sat(sat.low_u64()).into())
        } else {
            None
        }
    }

    /// Convert the amount represented in wei into an [bitcoin::Amount].
    ///
    /// Returns [None] if the wei amount exceeds the maximum value of
    /// [bitcoin::Amount::max_value()] or if the given wei amount is not an
    /// exact multiple of one satoshi.
    fn from_wei(wei: U256) -> Option<Self> {
        let ret = Self::from_wei_floor(wei)?;
        if ret.to_wei() == wei {
            Some(ret)
        } else {
            None
        }
    }
}
impl AmountExt for bitcoin::Amount {}


#[cfg(test)]
mod test {
    use super::*;

    use bitcoin::Amount;

    #[test]
    fn test_satoshi_in_wei() {
        assert_eq!(SATOSHI_IN_WEI, U256::from(10_i64.pow(10)));
        assert_eq!(Amount::max_value().to_wei(), MAX_SATOSHI * SATOSHI_IN_WEI);
    }

    #[test]
    fn test_amount_wei_conversion() {
        let max = Amount::max_value();
        assert_eq!(max, Amount::from_wei(max.to_wei()).unwrap());
        assert!(Amount::from_wei_floor(max.to_wei() + SATOSHI_IN_WEI).is_none());

        let some_wei = Amount::from_sat(350).to_wei();
        assert_eq!(Amount::from_wei(some_wei).unwrap(), Amount::from_sat(350));
        assert!(Amount::from_wei(some_wei + 1).is_none());
        assert_eq!(Amount::from_wei_floor(some_wei + 1).unwrap(), Amount::from_sat(350));
    }
}
