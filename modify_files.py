import sys
import re

# Update lib.rs
try:
    with open('adk-gemini/src/lib.rs', 'r') as f:
        content = f.read()

    if "pub mod vertex;" not in content:
        content = content.replace("pub mod tools;", "pub mod tools;\n\n/// Vertex AI specific types and configuration\n#[cfg(feature = \"vertex\")]\npub mod vertex;")

    with open('adk-gemini/src/lib.rs', 'w') as f:
        f.write(content)
    print("Updated adk-gemini/src/lib.rs")
except Exception as e:
    print(f"Error updating lib.rs: {e}")

# Update client.rs
try:
    with open('adk-gemini/src/client.rs', 'r') as f:
        content = f.read()

    # Remove VertexContext struct
    # Note: re.DOTALL makes . match newlines
    pattern = r'/// Context for Vertex AI authentication\.\s*#\[cfg\(feature = "vertex"\)\]\s*#\[derive\(Debug, Clone\)\]\s*pub struct VertexContext\s*\{\s*/// Google Cloud Project ID\.\s*pub project: String,\s*/// GCP Location \(e\.g\., "us-central1"\)\.\s*pub location: String,\s*/// OAuth2 Access Token\.\s*pub token: String,\s*\}'

    new_content = re.sub(pattern, "", content, flags=re.DOTALL)

    if content == new_content:
        print("Warning: VertexContext struct not found or pattern mismatch in client.rs")
    else:
        content = new_content
        print("Removed VertexContext struct from client.rs")

    # Add import
    if "use crate::vertex::VertexContext;" not in content:
        # Find a good place to insert. After crate imports.
        # "use crate::models::*;" is usually there.
        content = content.replace("use crate::models::*;", "use crate::models::*;\n#[cfg(feature = \"vertex\")]\nuse crate::vertex::VertexContext;")
        print("Added VertexContext import to client.rs")

    with open('adk-gemini/src/client.rs', 'w') as f:
        f.write(content)
except Exception as e:
    print(f"Error updating client.rs: {e}")
