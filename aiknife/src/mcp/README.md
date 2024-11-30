# Model Context Protocol (MCP) Supprt

- JSON Schema definition: https://github.com/modelcontextprotocol/specification/blob/main/schema/schema.json
- Typescript definition: https://github.com/modelcontextprotocol/specification/blob/main/schema/schema.ts
- JSON RPC spec: https://www.jsonrpc.org/specification
- Inpsector for testing MCP servers: https://github.com/modelcontextprotocol/inspector
- Typify tool to generate Rust from JSON RPC: https://github.com/oxidecomputer/typify


## Generating bindings

For now, I forked `typify` to fix a bug that results in invalid Rust code generated for MCP bindings (PR is https://github.com/oxidecomputer/typify/pull/705)

So this requires that my fork of `cargo-typify` is installed.  See the `Justfile`


