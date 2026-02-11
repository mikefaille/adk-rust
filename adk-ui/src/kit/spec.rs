use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Specification for a UI Kit.
///
/// This struct defines the visual language and component configuration for a UI system.
/// It is used by the `KitGenerator` to produce design tokens and component definitions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KitSpec {
    /// The name of the UI kit (e.g., "Corporate Dark").
    pub name: String,

    /// The semantic version of the kit (e.g., "1.0.0").
    pub version: String,

    /// Brand identity configuration.
    pub brand: KitBrand,

    /// Color palette configuration.
    pub colors: KitColors,

    /// Typography configuration.
    pub typography: KitTypography,

    /// Density setting for spacing and sizing.
    #[serde(default)]
    pub density: KitDensity,

    /// Border radius configuration.
    #[serde(default)]
    pub radius: KitRadius,

    /// Component-specific overrides.
    #[serde(default)]
    pub components: KitComponents,

    /// List of template names to include in the kit.
    #[serde(default)]
    pub templates: Vec<String>,
}

impl Default for KitSpec {
    fn default() -> Self {
        Self {
            name: "Default Kit".to_string(),
            version: "0.1.0".to_string(),
            brand: KitBrand { vibe: "neutral".to_string(), industry: None },
            colors: KitColors {
                primary: "#000000".to_string(),
                accent: None,
                surface: None,
                background: None,
                text: None,
            },
            typography: KitTypography { family: "system-ui".to_string(), scale: None },
            density: KitDensity::default(),
            radius: KitRadius::default(),
            components: KitComponents::default(),
            templates: Vec::new(),
        }
    }
}

/// Brand identity settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KitBrand {
    /// A description of the brand's personality (e.g., "friendly", "serious").
    pub vibe: String,

    /// The industry sector (optional).
    #[serde(default)]
    pub industry: Option<String>,
}

/// Color palette configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KitColors {
    /// The primary brand color (hex code).
    pub primary: String,

    /// An optional accent color.
    #[serde(default)]
    pub accent: Option<String>,

    /// The surface color (e.g., for cards).
    #[serde(default)]
    pub surface: Option<String>,

    /// The background color.
    #[serde(default)]
    pub background: Option<String>,

    /// The main text color.
    #[serde(default)]
    pub text: Option<String>,
}

/// Typography configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KitTypography {
    /// The font family name (e.g., "Inter", "Roboto").
    pub family: String,

    /// The typographic scale (optional).
    #[serde(default)]
    pub scale: Option<String>,
}

/// Density levels for UI spacing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum KitDensity {
    /// Dense layout with minimal spacing.
    Compact,

    /// Standard comfortable spacing (default).
    #[default]
    Comfortable,

    /// Generous spacing for a more open feel.
    Spacious,
}

/// Border radius presets.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum KitRadius {
    /// No rounded corners.
    None,

    /// Small border radius.
    Sm,

    /// Medium border radius (default).
    #[default]
    Md,

    /// Large border radius.
    Lg,

    /// Extra large border radius (pill-shaped).
    Xl,
}

/// Configuration for individual components.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct KitComponents {
    /// Button component configuration.
    #[serde(default)]
    pub button: Option<KitComponentButton>,

    /// Card component configuration.
    #[serde(default)]
    pub card: Option<KitComponentCard>,

    /// Input field configuration.
    #[serde(default)]
    pub input: Option<KitComponentInput>,

    /// Table component configuration.
    #[serde(default)]
    pub table: Option<KitComponentTable>,
}

/// Button component settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KitComponentButton {
    /// Available button variants (e.g., "filled", "outlined").
    #[serde(default)]
    pub variants: Vec<String>,
}

/// Card component settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KitComponentCard {
    /// Elevation level or shadow style.
    #[serde(default)]
    pub elevation: Option<String>,
}

/// Input component settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KitComponentInput {
    /// Input style variant.
    #[serde(default)]
    pub style: Option<String>,
}

/// Table component settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KitComponentTable {
    /// Whether to use striped rows by default.
    #[serde(default)]
    pub striped: Option<bool>,
}
