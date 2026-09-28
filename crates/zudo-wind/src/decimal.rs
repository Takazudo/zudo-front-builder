use std::fmt;

const MAX_SIGNIFICANT_DIGITS: usize = 18;

/// An exact base-10 decimal with no floating-point conversion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decimal {
    negative: bool,
    digits: String,
    scale: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecimalError {
    InvalidLiteral,
    TooManySignificantDigits,
    Overflow,
}

impl fmt::Display for DecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLiteral => f.write_str("invalid decimal literal"),
            Self::TooManySignificantDigits => {
                write!(
                    f,
                    "decimal exceeds {MAX_SIGNIFICANT_DIGITS} significant digits"
                )
            }
            Self::Overflow => {
                write!(
                    f,
                    "decimal product exceeds {MAX_SIGNIFICANT_DIGITS} significant digits"
                )
            }
        }
    }
}

impl std::error::Error for DecimalError {}

impl Decimal {
    pub fn parse(value: &str) -> Result<Self, DecimalError> {
        let bytes = value.as_bytes();
        if bytes.is_empty() {
            return Err(DecimalError::InvalidLiteral);
        }

        let (negative, unsigned) = match bytes[0] {
            b'-' => (true, &value[1..]),
            b'+' => (false, &value[1..]),
            _ => (false, value),
        };
        if unsigned.is_empty() {
            return Err(DecimalError::InvalidLiteral);
        }

        let mut parts = unsigned.split('.');
        let whole = parts.next().ok_or(DecimalError::InvalidLiteral)?;
        let fraction = parts.next();
        if parts.next().is_some()
            || whole.is_empty()
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(DecimalError::InvalidLiteral);
        }
        let fraction = fraction.unwrap_or("");
        if fraction.is_empty() && unsigned.contains('.')
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(DecimalError::InvalidLiteral);
        }

        let mut scale = fraction.len();
        let mut digits = String::with_capacity(whole.len() + scale);
        digits.push_str(whole);
        digits.push_str(fraction);
        let first_nonzero = digits.bytes().position(|byte| byte != b'0');
        if let Some(first_nonzero) = first_nonzero {
            digits.drain(..first_nonzero);
        } else {
            digits.clear();
            digits.push('0');
        }
        while scale > 0 && digits.ends_with('0') && digits != "0" {
            digits.pop();
            scale -= 1;
        }
        if digits.len() > MAX_SIGNIFICANT_DIGITS {
            return Err(DecimalError::TooManySignificantDigits);
        }

        let mut result = Self {
            negative: negative && digits != "0",
            digits,
            scale,
        };
        result.normalize();
        Ok(result)
    }

    pub fn multiply(&self, other: &Self) -> Result<Self, DecimalError> {
        let mut digits = multiply_digits(&self.digits, &other.digits);
        let scale = self
            .scale
            .checked_add(other.scale)
            .ok_or(DecimalError::Overflow)?;
        if digits == "0" {
            return Ok(Self {
                negative: false,
                digits,
                scale: 0,
            });
        }
        let mut result = Self {
            negative: self.negative ^ other.negative,
            digits: std::mem::take(&mut digits),
            scale,
        };
        result.normalize();
        if result.digits.len() > MAX_SIGNIFICANT_DIGITS {
            return Err(DecimalError::Overflow);
        }
        Ok(result)
    }

    pub fn is_zero(&self) -> bool {
        self.digits == "0"
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    fn normalize(&mut self) {
        while self.scale > 0 && self.digits != "0" && self.digits.ends_with('0') {
            self.digits.pop();
            self.scale -= 1;
        }
        if self.digits == "0" {
            self.negative = false;
            self.scale = 0;
        }
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        if self.scale == 0 {
            return f.write_str(&self.digits);
        }
        if self.digits.len() <= self.scale {
            f.write_str("0.")?;
            for _ in 0..(self.scale - self.digits.len()) {
                f.write_str("0")?;
            }
            return f.write_str(&self.digits);
        }
        let split = self.digits.len() - self.scale;
        f.write_str(&self.digits[..split])?;
        f.write_str(".")?;
        f.write_str(&self.digits[split..])
    }
}

/// A decimal CSS dimension used for exact scale multiplication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecimalDimension {
    pub magnitude: Decimal,
    pub unit: String,
}

impl DecimalDimension {
    pub fn parse(value: &str) -> Result<Self, DecimalError> {
        let bytes = value.as_bytes();
        let mut number_end = 0;
        if matches!(bytes.first(), Some(b'-' | b'+')) {
            number_end = 1;
        }
        let integer_start = number_end;
        while bytes.get(number_end).is_some_and(u8::is_ascii_digit) {
            number_end += 1;
        }
        if number_end == integer_start {
            return Err(DecimalError::InvalidLiteral);
        }
        if bytes.get(number_end) == Some(&b'.') {
            number_end += 1;
            let fraction_start = number_end;
            while bytes.get(number_end).is_some_and(u8::is_ascii_digit) {
                number_end += 1;
            }
            if number_end == fraction_start {
                return Err(DecimalError::InvalidLiteral);
            }
        }
        let magnitude = Decimal::parse(&value[..number_end])?;
        let unit = &value[number_end..];
        if !unit.is_empty() && unit != "%" && !unit.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return Err(DecimalError::InvalidLiteral);
        }
        Ok(Self {
            magnitude,
            unit: unit.to_ascii_lowercase(),
        })
    }

    pub fn multiply(&self, multiplier: &Decimal) -> Result<Self, DecimalError> {
        let magnitude = self.magnitude.multiply(multiplier)?;
        Ok(Self {
            magnitude,
            unit: self.unit.clone(),
        })
    }

    pub fn to_css(&self) -> String {
        if self.magnitude.is_zero() {
            "0".to_owned()
        } else {
            format!("{}{}", self.magnitude, self.unit)
        }
    }
}

fn multiply_digits(left: &str, right: &str) -> String {
    if left == "0" || right == "0" {
        return "0".to_owned();
    }
    let left_digits = left.bytes().rev().map(|byte| (byte - b'0') as u32);
    let right_digits = right.bytes().rev().map(|byte| (byte - b'0') as u32);
    let left_values: Vec<_> = left_digits.collect();
    let right_values: Vec<_> = right_digits.collect();
    let mut product = vec![0_u32; left_values.len() + right_values.len()];

    for (left_index, left_digit) in left_values.iter().enumerate() {
        for (right_index, right_digit) in right_values.iter().enumerate() {
            product[left_index + right_index] += left_digit * right_digit;
        }
    }
    for index in 0..product.len() - 1 {
        let carry = product[index] / 10;
        product[index] %= 10;
        product[index + 1] += carry;
    }
    while product.last() == Some(&0) {
        product.pop();
    }
    product
        .iter()
        .rev()
        .map(|digit| char::from(b'0' + (*digit as u8)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Decimal, DecimalDimension, DecimalError};

    #[test]
    fn exact_spacing_unit_multiples_have_canonical_output() {
        let unit = DecimalDimension::parse("0.25rem").expect("valid dimension");
        for (multiple, expected) in [
            ("0.5", "0.125rem"),
            ("1.5", "0.375rem"),
            ("14", "3.5rem"),
            ("48", "12rem"),
        ] {
            let result = unit
                .multiply(&Decimal::parse(multiple).expect("valid multiplier"))
                .expect("exact product");
            assert_eq!(result.to_css(), expected);
        }
    }

    #[test]
    fn decimal_multiplication_does_not_drift() {
        let unit = DecimalDimension::parse("0.1rem").expect("valid dimension");
        let result = unit
            .multiply(&Decimal::parse("3").expect("valid multiplier"))
            .expect("exact product");
        assert_eq!(result.to_css(), "0.3rem");
    }

    #[test]
    fn decimals_normalize_sign_zero_and_trailing_zeroes() {
        assert_eq!(Decimal::parse("001.2300").unwrap().to_string(), "1.23");
        assert_eq!(Decimal::parse("-0.000").unwrap().to_string(), "0");
        assert_eq!(Decimal::parse("0.00012").unwrap().to_string(), "0.00012");
    }

    #[test]
    fn precision_is_limited_to_eighteen_significant_digits() {
        assert!(matches!(
            Decimal::parse("1234567890123456789"),
            Err(DecimalError::TooManySignificantDigits)
        ));
        let left = Decimal::parse("999999999999999999").unwrap();
        let right = Decimal::parse("9").unwrap();
        assert_eq!(left.multiply(&right), Err(DecimalError::Overflow));
    }

    #[test]
    fn dimension_parser_rejects_non_literal_values() {
        for invalid in [".5rem", "1.", "calc(1px)", "1px 2px", "1%foo"] {
            assert!(DecimalDimension::parse(invalid).is_err(), "{invalid}");
        }
    }
}
