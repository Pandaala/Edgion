//! Route hostname-list validation owned by the resource schema layer.

use edgion_algo::matcher::radix_hostname::{RadixHostnamePatternError, RadixHostnamePatternRef};
use std::{error::Error, fmt};

/// A rejected configured Route hostname list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostnameError {
    InvalidPattern,
    InvalidLength,
    IpLiteral,
    MissingHostnames,
    TooManyHostnames,
}

impl From<RadixHostnamePatternError> for HostnameError {
    fn from(value: RadixHostnamePatternError) -> Self {
        match value {
            RadixHostnamePatternError::InvalidPattern => Self::InvalidPattern,
            RadixHostnamePatternError::InvalidLength => Self::InvalidLength,
            RadixHostnamePatternError::IpLiteral => Self::IpLiteral,
        }
    }
}

impl fmt::Display for HostnameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPattern => "hostnames must use lowercase DNS labels with only an optional leading '*.'",
            Self::InvalidLength => "hostnames must contain between 1 and 253 bytes",
            Self::IpLiteral => "hostnames must not be IP address literals",
            Self::MissingHostnames => "TLSRoute requires at least one hostname",
            Self::TooManyHostnames => "routes support at most 16 hostnames",
        })
    }
}

impl Error for HostnameError {}

/// Validate the entire authored list; invalid entries never become catch-all.
/// HTTPRoute/GRPCRoute permit omitted or empty lists; TLSRoute requires 1..=16.
pub fn validate_route_hostnames(hostnames: Option<&[String]>, required: bool) -> Result<(), HostnameError> {
    let hostnames = hostnames.unwrap_or_default();
    if required && hostnames.is_empty() {
        return Err(HostnameError::MissingHostnames);
    }
    if hostnames.len() > 16 {
        return Err(HostnameError::TooManyHostnames);
    }
    for hostname in hostnames {
        RadixHostnamePatternRef::parse(hostname).map_err(HostnameError::from)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_hostname_lists_are_all_or_nothing() {
        for required in [false, true] {
            assert_eq!(validate_route_hostnames(None, required).is_ok(), !required);
            assert_eq!(validate_route_hostnames(Some(&[]), required).is_ok(), !required);
            assert!(validate_route_hostnames(Some(&vec!["a".into(); 16]), required).is_ok());
            assert!(validate_route_hostnames(Some(&vec!["a".into(); 17]), required).is_err());
            assert!(validate_route_hostnames(Some(&["a".into(), "*".into()]), required).is_err());
        }
    }
}
