use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{CoreError, CoreResult};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum RealmId {
    Live,
    Lab(String),
}

impl RealmId {
    pub fn storage_key(&self) -> String {
        match self {
            Self::Live => "live".to_owned(),
            Self::Lab(id) => format!("lab:{id}"),
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }
}

impl fmt::Display for RealmId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.storage_key())
    }
}

impl FromStr for RealmId {
    type Err = CoreError;

    fn from_str(value: &str) -> CoreResult<Self> {
        if value == "live" {
            return Ok(Self::Live);
        }

        let id = value.strip_prefix("lab:").ok_or_else(|| {
            CoreError::InvalidRealm("use 'live' or 'lab:<identifier>'".to_owned())
        })?;

        let valid = !id.is_empty()
            && id.len() <= 32
            && id.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            });
        if !valid {
            return Err(CoreError::InvalidRealm(
                "lab identifiers must contain 1-32 lowercase letters, digits, or hyphens"
                    .to_owned(),
            ));
        }

        Ok(Self::Lab(id.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::RealmId;

    #[test]
    fn accepts_live_and_sanitized_lab_realms() {
        assert_eq!(RealmId::from_str("live").unwrap(), RealmId::Live);
        assert_eq!(
            RealmId::from_str("lab:failure-injection").unwrap(),
            RealmId::Lab("failure-injection".to_owned())
        );
    }

    #[test]
    fn rejects_path_like_lab_identifiers() {
        assert!(RealmId::from_str("lab:../live").is_err());
        assert!(RealmId::from_str("lab:MixedCase").is_err());
        assert!(RealmId::from_str("sandbox").is_err());
    }
}
