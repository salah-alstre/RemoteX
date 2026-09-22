use serde::Deserialize;

/// Branding lives in `branding.json` at the repository root so it can be changed in one place.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branding {
    pub name: String,
    pub identifier: String,
    pub company: String,
    pub website: String,
    pub github: String,
    pub support_email: String,
}

impl Branding {
    pub fn load() -> Self {
        serde_json::from_str(include_str!("../../../branding.json")).expect("branding.json is valid")
    }
}
