# Install the Typify tooling to use to codegen the Rust bindings
# (for now, installs my own custom fork with a key for for MCP support)
install-typify:
    cargo install cargo-typify --force --git "https://github.com/anelson/typify.git" --branch "fix/mcp-json-schema-fixes"

# Generate Rust types from the MCP JSON Schema
mcpcodegen:
    cd aiknife && cargo typify --output src/mcp/types.rs  src/mcp/schema.json

