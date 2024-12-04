use anyhow::Result;
use jsonrpsee::types as jsonrpc;
use std::borrow::Cow;
use tracing::*;

/// Possible kinds of request from JSON-RPC clients
#[derive(Debug)]
pub(crate) enum JsonRpcRequest<'a> {
    /// A regular method invocation
    Request(jsonrpc::Request<'a>),

    /// A notification, which is fire-and-forget and does not elicit a response
    ///
    /// NOTE: this weird ugly `Option<..>` crap is copied from `server.js` in the jsonrpsee-server
    /// code.  It allows to defer the parsing of the payload until later when we can determine what
    /// the expected input is.
    Notification(jsonrpc::Notification<'a, Option<&'a serde_json::value::RawValue>>),

    /// An invalid request, which is a JSON-RPC error, but still has an ID field so that when we
    /// report the error we can include the ID of the request that caused it.
    InvalidRequest(jsonrpc::InvalidRequest<'a>),
}

impl<'a> JsonRpcRequest<'a> {
    pub(crate) fn from_str(request: &'a str) -> Result<Self, JsonRpcError> {
        // Inspired by the `handle_rpc_call` function in jsonrpsee-server in `src/server.rs`

        // In short: try to parse as jsonrpc::Request, if not then as Notification, and if not as
        // InvalidRequest
        if let Ok(request) = serde_json::from_str::<jsonrpc::Request>(request) {
            Ok(JsonRpcRequest::Request(request))
        } else if let Ok(notification) = serde_json::from_str::<
            jsonrpc::Notification<'a, Option<&'a serde_json::value::RawValue>>,
        >(request)
        {
            Ok(JsonRpcRequest::Notification(notification))
        } else {
            Ok(JsonRpcRequest::InvalidRequest(
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

impl Into<jsonrpc::Response<'static, serde_json::Value>> for JsonRpcError {
    fn into(self) -> jsonrpc::Response<'static, serde_json::Value> {
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
        let request: JsonRpcRequest = JsonRpcRequest::from_str(request).unwrap();
        assert!(matches!(request, JsonRpcRequest::Notification(_)));
    }
}
