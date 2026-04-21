use serde::{Deserialize, Serialize};

use crate::TrustLevel;

/// Structured description of a site's business domain, capabilities, and policies.
///
/// Parsed from a `business.toml` file and used to auto-generate discovery documents
/// and capability manifests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusinessContext {
    pub site_name: String,
    pub site_description: String,
    pub domain: String,
    pub capabilities: Vec<BusinessCapability>,
    pub policies: Vec<BusinessPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
}

/// A business capability with access level requirements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusinessCapability {
    pub name: String,
    pub description: String,
    pub endpoint: String,
    pub method: String,
    pub access_level: TrustLevel,
}

/// A business policy describing operational rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusinessPolicy {
    pub name: String,
    pub description: String,
    pub policy_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_context() -> BusinessContext {
        BusinessContext {
            site_name: "Test Site".to_string(),
            site_description: "A test site".to_string(),
            domain: "example.com".to_string(),
            capabilities: vec![BusinessCapability {
                name: "read_data".to_string(),
                description: "Read data".to_string(),
                endpoint: "/api/data".to_string(),
                method: "GET".to_string(),
                access_level: TrustLevel::Anonymous,
            }],
            policies: vec![BusinessPolicy {
                name: "privacy".to_string(),
                description: "Privacy policy".to_string(),
                policy_type: "privacy".to_string(),
            }],
            contact: Some("admin@example.com".to_string()),
        }
    }

    #[test]
    fn test_json_serde_round_trip() {
        let ctx = sample_context();
        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: BusinessContext = serde_json::from_str(&json).unwrap();
        assert_eq!(ctx, deserialized);
    }

    #[test]
    fn test_toml_serde_round_trip() {
        let ctx = sample_context();
        let toml_str = toml::to_string(&ctx).unwrap();
        let deserialized: BusinessContext = toml::from_str(&toml_str).unwrap();
        assert_eq!(ctx, deserialized);
    }

    #[test]
    fn test_optional_contact_skipped() {
        let ctx = BusinessContext {
            site_name: "Test".to_string(),
            site_description: "Test".to_string(),
            domain: "example.com".to_string(),
            capabilities: vec![],
            policies: vec![],
            contact: None,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(!json.contains("contact"));
    }
}
