//! # UI Kit Generation
//!
//! This module provides tools for generating UI kits and design system specifications for ADK-UI.
//!
//! A UI Kit defines the visual language (colors, typography, density, etc.) and component styles
//! used by the UI renderer. The `KitGenerator` takes a high-level `KitSpec` and produces a set of
//! artifacts including a component catalog, design tokens, and CSS variables.
//!
//! ## Usage
//!
//! ```rust
//! use adk_ui::kit::{KitGenerator, KitSpec, KitBrand, KitColors, KitTypography};
//!
//! let spec = KitSpec {
//!     name: "My Kit".to_string(),
//!     version: "1.0.0".to_string(),
//!     brand: KitBrand {
//!         vibe: "professional".to_string(),
//!         industry: None,
//!     },
//!     colors: KitColors {
//!         primary: "#007bff".to_string(),
//!         accent: None,
//!         surface: None,
//!         background: None,
//!         text: None,
//!     },
//!     typography: KitTypography {
//!         family: "Inter".to_string(),
//!         scale: None,
//!     },
//!     ..Default::default()
//! };
//!
//! let artifacts = KitGenerator::new().generate(&spec);
//! println!("Generated catalog with ID: {}", artifacts.catalog["catalogId"]);
//! ```

pub mod generator;
pub mod spec;

pub use generator::{KitArtifacts, KitGenerator};
pub use spec::{KitBrand, KitColors, KitComponents, KitDensity, KitRadius, KitSpec, KitTypography};
