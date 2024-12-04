use anyhow::Result;
use jsonrpsee_types as jsonrpc;

/// Re-use some of the heavy lifting done in jsonrpsee, pretending as if these are our own types
pub(crate) use jsonrpc::{ErrorCode, ErrorObjectOwned, Id, Request, Response, ResponsePayload};

/// Type that tells `serde_json` that we expect a valid JSON value, but we want to defer parsing it
/// until later.  This is used in the JSON RPC impl code where we don't yet know what specific Rust
/// type a method or notification takes and don't want to descent into type parameter hell.
///
/// `serde_json::value::RawValue` is a special case type with specific optimizations in `serde_json`
pub(crate) type GenericParams<'a> = &'a serde_json::value::RawValue;

/// Convenient type alias for notifications with generic raw JSON payloads.
///
/// The jsonrpsee `Request` type explicitly holds only a raw JSON payload, for some reason the
/// Notification type doesn't.  That is what we need here
///
/// `serde_json::value::RawValue` is a special case type that contains valid JSON but is just a
/// reference to the slice of the input string containing that JSON.  This lets us use a generic
/// type here without incurring the cost of parsing JSON to `serde_json::Value` and then
/// re-processing that again into some expected type.
pub(crate) type Notification<'a> = jsonrpc::Notification<'a, Option<GenericParams<'a>>>;

/// The response type that has a generic JSON payload.  The actual type of the payload is
/// method-specific and is not known at the level of the JSON-RPC impl
pub(crate) type GenericResponse = Response<'static, serde_json::Value>;

/// Possible kinds of messages sent to servers from JSON-RPC clients
#[derive(Debug)]
pub(crate) enum JsonRpcClientMessage<'a> {
    /// A regular method invocation
    Request(jsonrpc::Request<'a>),

    /// A notification, which is fire-and-forget and does not elicit a response
    Notification(Notification<'a>),

    /// An invalid request, which is a JSON-RPC error, but still has an ID field so that when we
    /// report the error we can include the ID of the request that caused it.
    InvalidRequest(jsonrpc::InvalidRequest<'a>),
}

impl<'a> JsonRpcClientMessage<'a> {
    pub(crate) fn from_str(request: &'a str) -> Result<Self, JsonRpcError> {
        // Inspired by the `handle_rpc_call` function in jsonrpsee-server in `src/server.rs`

        // In short: try to parse as jsonrpc::Request, if not then as Notification, and if not as
        // InvalidRequest
        if let Ok(request) = serde_json::from_str::<jsonrpc::Request>(request) {
            Ok(JsonRpcClientMessage::Request(request))
        } else if let Ok(notification) = serde_json::from_str::<Notification>(request) {
            Ok(JsonRpcClientMessage::Notification(notification))
        } else {
            Ok(JsonRpcClientMessage::InvalidRequest(
                serde_json::from_str::<jsonrpc::InvalidRequest>(request)
                    .map_err(|e| JsonRpcError::deser(e, None))?,
            ))
        }
    }
}

/// Internal error type that captures errors as JSON-RPC errors
///
/// Used only to capture error information in enough detail to generate proper JSON error
/// responses.
///
/// Uses the error codes defined in the JSON-RPC spec.
#[derive(Debug)]
pub(crate) struct JsonRpcError {
    code: jsonrpc::ErrorCode,
    message: String,
    id: Option<jsonrpc::Id<'static>>,
    data: Option<Vec<String>>,
}

impl JsonRpcError {
    fn new(
        code: jsonrpc::ErrorCode,
        message: impl Into<String>,
        id: impl Into<Option<jsonrpc::Id<'static>>>,
        data: impl Into<Option<Vec<String>>>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            id: id.into(),
            data: data.into(),
        }
    }

    /// Make a new JSON RPC error, capturing the source error chain in the `data` field
    fn from_error(
        code: jsonrpc::ErrorCode,
        error: impl std::error::Error,
        id: impl Into<Option<jsonrpc::Id<'static>>>,
    ) -> Self {
        let message = error.to_string();
        let mut inner = error.source();
        let mut data = vec![];
        while let Some(e) = inner {
            data.push(e.to_string());
            inner = e.source();
        }

        Self::new(code, message, id, data)
    }

    /// Error deserializing some JSON.
    pub(crate) fn deser(
        error: serde_json::Error,
        id: impl Into<Option<jsonrpc::Id<'static>>>,
    ) -> Self {
        Self::from_error(jsonrpc::ErrorCode::ParseError, error, id)
    }

    /// Error serializing some JSON.  Less likely but still possible.
    pub(crate) fn ser(error: serde_json::Error, id: jsonrpc::Id<'static>) -> Self {
        Self::from_error(jsonrpc::ErrorCode::ParseError, error, id)
    }

    pub(crate) fn method_not_found(method: String, id: jsonrpc::Id<'static>) -> Self {
        Self::new(
            jsonrpc::ErrorCode::MethodNotFound,
            format!("Method not found: {}", method),
            id,
            None,
        )
    }

    pub(crate) fn invalid_params(id: jsonrpc::Id<'static>) -> Self {
        Self::new(
            jsonrpc::ErrorCode::InvalidParams,
            jsonrpc::ErrorCode::InvalidParams.message(),
            id,
            None,
        )
    }

    pub(crate) fn invalid_request(id: jsonrpc::Id<'static>) -> Self {
        Self::new(
            jsonrpc::ErrorCode::InvalidRequest,
            jsonrpc::ErrorCode::InvalidRequest.message(),
            id,
            None,
        )
    }
}

/// Implement the conversion from `JsonRpcError` to a JSON-RPC response.
///
/// This is a convenience method to allow `JsonRpcError` to be used directly as a response
/// elsewhere in the implementation.
impl Into<GenericResponse> for JsonRpcError {
    fn into(self) -> GenericResponse {
        jsonrpc::Response::new(
            jsonrpc::ResponsePayload::error(jsonrpc::ErrorObjectOwned::owned::<Vec<String>>(
                self.code.code(),
                self.message,
                self.data,
            )),
            self.id.unwrap_or(jsonrpc::Id::Null),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_rpc_notification() {
        let request = r#"    { "jsonrpc": "2.0", "method": "notifications/initialized" }"#;
        let request: JsonRpcClientMessage = JsonRpcClientMessage::from_str(request).unwrap();
        assert!(matches!(request, JsonRpcClientMessage::Notification(_)));
    }
}
