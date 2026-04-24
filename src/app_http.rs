use std::{any::type_name, future::Future, io::Read, pin::Pin};

use gpui::http_client::{self, HttpClient};

pub struct UreqHttpClient {
    agent: ureq::Agent,
    user_agent: http_client::http::HeaderValue,
}

impl UreqHttpClient {
    pub fn new() -> Self {
        Self {
            agent: ureq::Agent::new(),
            user_agent: http_client::http::HeaderValue::from_static("gh-ui/0.1"),
        }
    }
}

impl HttpClient for UreqHttpClient {
    fn send(
        &self,
        req: http_client::Request<http_client::AsyncBody>,
    ) -> Pin<
        Box<
            dyn Future<Output = http_client::Result<http_client::Response<http_client::AsyncBody>>>
                + Send
                + 'static,
        >,
    > {
        let agent = self.agent.clone();
        let user_agent = self.user_agent.clone();

        Box::pin(async move { send_with_ureq(agent, req, user_agent) })
    }

    fn user_agent(&self) -> Option<&http_client::http::HeaderValue> {
        Some(&self.user_agent)
    }

    fn proxy(&self) -> Option<&http_client::Url> {
        None
    }

    fn type_name(&self) -> &'static str {
        type_name::<Self>()
    }
}

fn send_with_ureq(
    agent: ureq::Agent,
    req: http_client::Request<http_client::AsyncBody>,
    user_agent: http_client::http::HeaderValue,
) -> http_client::Result<http_client::Response<http_client::AsyncBody>> {
    use ureq::OrAnyStatus;

    let (parts, _body) = req.into_parts();
    let uri = parts.uri.to_string();
    let mut request = agent.request(parts.method.as_str(), &uri);

    if !parts
        .headers
        .contains_key(http_client::http::header::USER_AGENT)
    {
        request = request.set("User-Agent", user_agent.to_str()?);
    }

    for (name, value) in &parts.headers {
        if let Ok(value) = value.to_str() {
            request = request.set(name.as_str(), value);
        }
    }

    let response = request.call().or_any_status()?;
    let status = response.status();
    let header_names = response.headers_names();

    let mut builder = http_client::Response::builder().status(status);
    for name in header_names {
        for value in response.all(&name) {
            builder = builder.header(name.as_str(), value);
        }
    }

    let mut body = Vec::new();
    response.into_reader().read_to_end(&mut body)?;

    builder
        .body(http_client::AsyncBody::from(body))
        .map_err(Into::into)
}
