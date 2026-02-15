import re

file_path = 'adk-gemini/src/client.rs'

with open(file_path, 'r') as f:
    content = f.read()

# 1. Guard imports
imports_to_guard = [
    r'use google_cloud_aiplatform_v1::client::PredictionService;',
    r'use google_cloud_auth::credentials::\{self, Credentials\};',
    r'use google_cloud_auth::credentials::Credentials;',
]

for imp in imports_to_guard:
    # Escape for regex but keep structure
    pattern = re.escape(imp).replace(r'\{', '{').replace(r'\}', '}')
    # If using regex, escape special chars. Simple replace is safer for exact strings.
    # But imports might be formatted differently.
    # Let's try simple replacement first if they match exactly.
    pass

# Actual replacements
content = content.replace(
    'use google_cloud_aiplatform_v1::client::PredictionService;',
    '#[cfg(feature = "vertex")]\nuse google_cloud_aiplatform_v1::client::PredictionService;'
)
content = content.replace(
    'use google_cloud_auth::credentials::{self, Credentials};',
    '#[cfg(feature = "vertex")]\nuse google_cloud_auth::credentials::{self, Credentials};'
)

# 2. Guard Error variants
# We need to guard specifically the google cloud ones.
error_variants = [
    (r'GoogleCloudAuth \{ source: google_cloud_auth::build_errors::Error \},', 'GoogleCloudAuth'),
    (r'GoogleCloudCredentialHeaders \{ source: google_cloud_auth::errors::CredentialsError \},', 'GoogleCloudCredentialHeaders'),
    (r'GoogleCloudCredentialHeadersUnavailable,', 'GoogleCloudCredentialHeadersUnavailable'),
    (r'GoogleCloudCredentialParse \{ source: serde_json::Error \},', 'GoogleCloudCredentialParse'),
    (r'GoogleCloudClientBuild \{ source: google_cloud_gax::client_builder::Error \},', 'GoogleCloudClientBuild'),
    (r'GoogleCloudRequest \{ source: google_cloud_aiplatform_v1::Error \},', 'GoogleCloudRequest'),
    (r'GoogleCloudRequestSerialize \{ source: serde_json::Error \},', 'GoogleCloudRequestSerialize'),
    (r'GoogleCloudRequestDeserialize \{ source: serde_json::Error \},', 'GoogleCloudRequestDeserialize'),
    (r'GoogleCloudResponseSerialize \{ source: serde_json::Error \},', 'GoogleCloudResponseSerialize'),
    (r'GoogleCloudResponseDeserialize \{ source: serde_json::Error \},', 'GoogleCloudResponseDeserialize'),
    (r'GoogleCloudRequestNotObject,', 'GoogleCloudRequestNotObject'),
    (r'MissingGoogleCloudConfig,', 'MissingGoogleCloudConfig'),
    (r'MissingGoogleCloudAuth,', 'MissingGoogleCloudAuth'),
    (r'MissingGoogleCloudProjectId,', 'MissingGoogleCloudProjectId'),
    (r'GoogleCloudUnsupported \{ operation: &\'static str \},', 'GoogleCloudUnsupported'),
    (r'TokioRuntime \{ source: std::io::Error \},', 'TokioRuntime'),
    (r'GoogleCloudInitThreadPanicked,', 'GoogleCloudInitThreadPanicked'),
]

for pattern, name in error_variants:
    # Use simple replace if possible, but indentation matters.
    # We replace the line with #[cfg(feature = "vertex")]\n<line>
    # Note: Regex is better to capture indentation.
    regex_pattern = r'(\s+)' + re.escape(pattern)
    replacement = r'\1#[cfg(feature = "vertex")]\n\1' + pattern
    content = re.sub(regex_pattern, replacement, content)

# 3. Guard GeminiClient::with_vertex
content = content.replace(
    '    /// Create a client backed by Vertex AI.\n    fn with_vertex',
    '    /// Create a client backed by Vertex AI.\n    #[cfg(feature = "vertex")]\n    fn with_vertex'
)

# 4. Guard GoogleCloudAuth enum and related structs/functions
# Since they are used in GeminiBuilder which we modify, we can guard the whole blocks.
content = content.replace(
    '#[derive(Debug, Clone)]\nenum GoogleCloudAuth {',
    '#[derive(Debug, Clone)]\n#[cfg(feature = "vertex")]\nenum GoogleCloudAuth {'
)

content = content.replace(
    '#[derive(Debug, Clone)]\nstruct GoogleCloudConfig {',
    '#[derive(Debug, Clone)]\n#[cfg(feature = "vertex")]\nstruct GoogleCloudConfig {'
)

content = content.replace(
    'fn extract_service_account_project_id(',
    '#[cfg(feature = "vertex")]\nfn extract_service_account_project_id('
)

content = content.replace(
    'fn build_vertex_prediction_service(',
    '#[cfg(feature = "vertex")]\nfn build_vertex_prediction_service('
)

# 5. Guard GeminiBuilder fields
content = content.replace(
    '    google_cloud: Option<GoogleCloudConfig>,',
    '    #[cfg(feature = "vertex")]\n    google_cloud: Option<GoogleCloudConfig>,'
)
content = content.replace(
    '    api_key: Option<String>,',
    '    api_key: Option<String>,'
)
content = content.replace(
    '    google_cloud_auth: Option<GoogleCloudAuth>,',
    '    #[cfg(feature = "vertex")]\n    google_cloud_auth: Option<GoogleCloudAuth>,'
)

# 6. Guard GeminiBuilder::new initialization
# We need to modify new() to initialize guarded fields only if feature enabled.
# Or better, cfg block inside new struct init?
# Rust struct init allows attributes on fields.
content = content.replace(
    '            google_cloud: None,',
    '            #[cfg(feature = "vertex")]\n            google_cloud: None,'
)
content = content.replace(
    '            google_cloud_auth: None,',
    '            #[cfg(feature = "vertex")]\n            google_cloud_auth: None,'
)

# 7. Guard GeminiBuilder methods
methods_to_guard = [
    'pub fn with_service_account_json',
    'pub fn with_google_cloud<P:',
    'pub fn with_google_cloud_adc',
    'pub fn with_google_cloud_wif_json'
]

for method in methods_to_guard:
    content = content.replace(
        method,
        '#[cfg(feature = "vertex")]\n    ' + method
    )

# 8. Guard GeminiBuilder::with_base_url (it resets google_cloud fields)
# This one is tricky because it accesses fields that might not exist.
# We need to guard the lines inside.
with_base_url_body = r'''        self.base_url = base_url;
        self.google_cloud = None;
        self.google_cloud_auth = None;
        self'''
with_base_url_body_fixed = r'''        self.base_url = base_url;
        #[cfg(feature = "vertex")]
        {
            self.google_cloud = None;
            self.google_cloud_auth = None;
        }
        self'''
content = content.replace(with_base_url_body, with_base_url_body_fixed)

# 9. Guard GeminiBuilder::build Vertex path
# The Vertex path starts with `if let Some(config) = &self.google_cloud {`
# If we guard `google_cloud` field, we must guard this access.
# Since `google_cloud` field only exists with feature, we can guard the whole block.
# BUT `if let` implies optionality.
# We can wrap the block in `#[cfg(feature = "vertex")]`.
# Finding the block is hard with replace.
# But we can replace the if statement line.
content = content.replace(
    '        if self.google_cloud.is_none() && self.google_cloud_auth.is_some() {',
    '        #[cfg(feature = "vertex")]\n        if self.google_cloud.is_none() && self.google_cloud_auth.is_some() {'
)

content = content.replace(
    '        // ── Vertex AI path ──────────────────────────────────────────────\n        if let Some(config) = &self.google_cloud {',
    '        // ── Vertex AI path ──────────────────────────────────────────────\n        #[cfg(feature = "vertex")]\n        if let Some(config) = &self.google_cloud {'
)

# 10. Guard Gemini constructors
gemini_methods = [
    'pub fn with_google_cloud<K:',
    'pub fn with_google_cloud_model<K:',
    'pub fn with_google_cloud_adc<P:',
    'pub fn with_google_cloud_adc_model<P:',
    'pub fn with_google_cloud_wif_json<P:',
    'pub fn with_service_account_json(',
    'pub fn with_service_account_json_model<M:',
    'pub fn with_google_cloud_service_account_json<M:',
]

for method in gemini_methods:
    content = content.replace(
        method,
        '#[cfg(feature = "vertex")]\n    ' + method
    )

# 11. Guard tests
content = content.replace(
    '    use crate::backend::vertex::VertexBackend;',
    '    #[cfg(feature = "vertex")]\n    use crate::backend::vertex::VertexBackend;'
)

content = content.replace(
    '    #[test]\n    fn extract_service_account_project_id_reads_project_id() {',
    '    #[test]\n    #[cfg(feature = "vertex")]\n    fn extract_service_account_project_id_reads_project_id() {'
)
content = content.replace(
    '    #[test]\n    fn extract_service_account_project_id_missing_field_errors() {',
    '    #[test]\n    #[cfg(feature = "vertex")]\n    fn extract_service_account_project_id_missing_field_errors() {'
)
content = content.replace(
    '    #[test]\n    fn extract_service_account_project_id_invalid_json_errors() {',
    '    #[test]\n    #[cfg(feature = "vertex")]\n    fn extract_service_account_project_id_invalid_json_errors() {'
)
content = content.replace(
    '    #[test]\n    fn vertex_transport_error_detection_matches_http2_failure() {',
    '    #[test]\n    #[cfg(feature = "vertex")]\n    fn vertex_transport_error_detection_matches_http2_failure() {'
)

with open(file_path, 'w') as f:
    f.write(content)
