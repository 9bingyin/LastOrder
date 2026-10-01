use super::*;

fn forbidden() -> Response {
    ApiError {
        status: StatusCode::FORBIDDEN,
        code: "permission_denied",
        message: "本地访问凭证或来源无效".into(),
    }
    .into_response()
}

fn valid_request(headers: &HeaderMap, authority: &str) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    if host != authority {
        return false;
    }
    if let Some(origin) = headers.get(header::ORIGIN)
        && origin.to_str().ok() != Some(format!("http://{authority}").as_str())
    {
        return false;
    }
    !headers
        .get("sec-fetch-site")
        .is_some_and(|value| value != "same-origin" && value != "none")
}

pub(super) async fn local_request(
    State(state): State<WebState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if !valid_request(request.headers(), &state.authority) {
        return forbidden();
    }
    next.run(request).await
}

pub(super) async fn authorize(
    State(state): State<WebState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let ws_token = request
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let mut protocols = value.split(',').map(str::trim);
            if protocols.next()? != "lastorder" {
                return None;
            }
            let token = protocols.next()?;
            if protocols.next().is_some() {
                return None;
            }
            Some(token)
        });
    if !token
        .or(ws_token)
        .is_some_and(|token| secret_matches(token, &state.token))
    {
        return forbidden();
    }
    next.run(request).await
}
