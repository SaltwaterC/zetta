//! The application's HTTP client, built on its first request rather than at
//! launch.
//!
//! Building a `ReqwestClient` starts a Tokio runtime and loads the system's TLS
//! root certificates: about 4 ms that every launch spent before its window could
//! open. Nothing makes a request during start-up (downloading a theme, or
//! resolving a `github:` recipient, happens later if at all), so the window no
//! longer waits for it. A client that cannot be built fails the requests that
//! needed it instead of stopping Zetta from starting.

use std::sync::OnceLock;

use futures::future::BoxFuture;
use gpui::http_client::{
    AsyncBody, HttpClient, Url,
    http::{self, HeaderValue, Response},
};
use reqwest_client::ReqwestClient;

pub(crate) struct LazyHttpClient {
    user_agent: &'static str,
    client: OnceLock<Result<ReqwestClient, String>>,
}

impl LazyHttpClient {
    pub(crate) fn new(user_agent: &'static str) -> Self {
        Self {
            user_agent,
            client: OnceLock::new(),
        }
    }

    fn client(&self) -> Result<&ReqwestClient, &str> {
        self.client
            .get_or_init(|| {
                ReqwestClient::user_agent(self.user_agent).map_err(|error| format!("{error:#}"))
            })
            .as_ref()
            .map_err(String::as_str)
    }

    #[cfg(test)]
    fn is_built(&self) -> bool {
        self.client.get().is_some()
    }
}

impl HttpClient for LazyHttpClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        self.client().ok()?.user_agent()
    }

    fn proxy(&self) -> Option<&Url> {
        self.client().ok()?.proxy()
    }

    fn send(
        &self,
        request: http::Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        match self.client() {
            Ok(client) => client.send(request),
            Err(error) => {
                let error = anyhow::anyhow!("initializing HTTP client: {error}");
                Box::pin(async move { Err(error) })
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/startup/http_client.rs"]
mod tests;
