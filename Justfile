# Install the Typify tooling to use to codegen the Rust bindings
# (for now, installs a specific commit in `main` that has my PR to fix MCP handling)
install-typify:
    cargo install cargo-typify --force --git "https://github.com/oxidecomputer/typify.git" --branch "main" --rev "f409d37"

# Generate Rust types from the MCP JSON Schema
mcpcodegen:
    cd aiknife && cargo typify --output src/mcp/types.rs  src/mcp/schema.json

