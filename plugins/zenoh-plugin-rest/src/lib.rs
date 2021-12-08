//
// Copyright (c) 2017, 2020 ADLINK Technology Inc.
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ADLINK zenoh team, <zenoh@adlink-labs.tech>
//

use async_std::sync::Arc;
use futures::prelude::*;
use http_types::Method;
use std::str::FromStr;
use tide::http::Mime;
use tide::sse::Sender;
use tide::{Request, Response, Server, StatusCode};
use zenoh::net::runtime::Runtime;
use zenoh::prelude::*;
use zenoh::query::{QueryConsolidation, ReplyReceiver};
use zenoh::Session;
use zenoh_plugin_trait::{prelude::*, RunningPluginTrait, ValidationFunction};
use zenoh_util::{bail, core::Result as ZResult};

const PORT_SEPARATOR: char = ':';
const DEFAULT_HTTP_HOST: &str = "0.0.0.0";
const DEFAULT_HTTP_PORT: &str = "8000";

fn parse_http_port(arg: &str) -> String {
    match arg.split(':').count() {
        1 => {
            match arg.parse::<u16>() {
                Ok(_) => [DEFAULT_HTTP_HOST, arg].join(&PORT_SEPARATOR.to_string()), // port only
                Err(_) => [arg, DEFAULT_HTTP_PORT].join(&PORT_SEPARATOR.to_string()), // host only
            }
        }
        _ => arg.to_string(),
    }
}

fn value_to_json(value: Value) -> String {
    // @TODO: transcode to JSON when implemented in Value
    match &value.encoding {
        p if p.starts_with(&Encoding::STRING) => {
            // convert to Json string for special characters escaping
            serde_json::json!(value.to_string()).to_string()
        }
        p if p.starts_with(&Encoding::APP_PROPERTIES) => {
            // convert to Json string for special characters escaping
            serde_json::json!(*crate::Properties::from(value.to_string())).to_string()
        }
        p if p.starts_with(&Encoding::APP_JSON) => value.to_string(),
        p if p.starts_with(&Encoding::APP_INTEGER) || p.starts_with(&Encoding::APP_FLOAT) => {
            value.to_string()
        }
        _ => {
            format!(r#""{}""#, base64::encode(value.payload.to_vec()))
        }
    }
}

fn sample_to_json(sample: Sample) -> String {
    let encoding = sample.value.encoding.to_string();
    format!(
        r#"{{ "key": "{}", "value": {}, "encoding": "{}", "time": "{}" }}"#,
        sample.key_expr.as_str(),
        value_to_json(sample.value),
        encoding,
        if let Some(ts) = sample.timestamp {
            ts.to_string()
        } else {
            "None".to_string()
        }
    )
}

async fn to_json(results: ReplyReceiver) -> String {
    let values = results
        .filter_map(move |reply| async move { Some(sample_to_json(reply.data)) })
        .collect::<Vec<String>>()
        .await
        .join(",\n");
    format!("[\n{}\n]\n", values)
}

fn sample_to_html(sample: Sample) -> String {
    format!(
        "<dt>{}</dt>\n<dd>{}</dd>\n",
        sample.key_expr.as_str(),
        String::from_utf8_lossy(&sample.value.payload.contiguous())
    )
}

async fn to_html(results: ReplyReceiver) -> String {
    let values = results
        .filter_map(move |reply| async move { Some(sample_to_html(reply.data)) })
        .collect::<Vec<String>>()
        .await
        .join("\n");
    format!("<dl>\n{}\n</dl>\n", values)
}

fn method_to_kind(method: Method) -> SampleKind {
    match method {
        Method::Put => SampleKind::Put,
        Method::Patch => SampleKind::Patch,
        Method::Delete => SampleKind::Delete,
        _ => SampleKind::default(),
    }
}

fn response(status: StatusCode, content_type: Mime, body: &str) -> Response {
    Response::builder(status)
        .header("content-length", body.len().to_string())
        .header("Access-Control-Allow-Origin", "*")
        .content_type(content_type)
        .body(body)
        .build()
}

zenoh_plugin_trait::declare_plugin!(RestPlugin);
pub struct RestPlugin {}
#[derive(Clone, Copy, Debug)]
struct StrError {
    err: &'static str,
}
impl std::error::Error for StrError {}
impl std::fmt::Display for StrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.err)
    }
}
#[derive(Debug, Clone)]
struct StringError {
    err: String,
}
impl std::error::Error for StringError {}
impl std::fmt::Display for StringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.err)
    }
}

impl Plugin for RestPlugin {
    fn compatibility() -> zenoh_plugin_trait::PluginId {
        zenoh_plugin_trait::PluginId {
            uid: "zenoh-plugin-rest",
        }
    }
    type StartArgs = Runtime;
    const STATIC_NAME: &'static str = "rest";

    fn start(name: &str, runtime: &Self::StartArgs) -> ZResult<Box<dyn RunningPluginTrait>> {
        let config = runtime.config.lock();
        let self_cfg: &serde_json::Value = match config.plugin(name) {
            Some(value) => value,
            None => {
                bail!("No configuration found for plugin '{}'", name)
            }
        };
        if let Some(port) = self_cfg.as_object().unwrap().get("port") {
            let port = match port {
                serde_json::Value::Null
                | serde_json::Value::Bool(_)
                | serde_json::Value::Array(_)
                | serde_json::Value::Object(_) => {
                    return Err(Box::new(StrError {
                        err: r#""port" option must be an integer"#,
                    }))
                }
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => {
                    if let Some(port) = n.as_u64() {
                        port.to_string()
                    } else {
                        return Err(Box::new(StrError {
                            err: r#""port" option must be a positive integer"#,
                        }));
                    }
                }
            };
            std::mem::drop(config);
            async_std::task::spawn(run(runtime.clone(), port));
            Ok(Box::new(RunningPlugin))
        } else {
            std::mem::drop(config);
            Err(Box::new(StrError {
                err: r#"Plugin option "port" is required"#,
            }))
        }
    }
}
struct RunningPlugin;
impl RunningPluginTrait for RunningPlugin {
    fn config_checker(&self) -> ValidationFunction {
        Arc::new(|_, _, _| {
            Err("zenoh-plugin-rest doesn't accept any runtime configuration changes".into())
        })
    }
}

async fn query(req: Request<(Arc<Session>, String)>) -> tide::Result<Response> {
    log::trace!("Incoming GET request: {:?}", req);

    let first_accept = match req.header("accept") {
        Some(accept) => accept[0]
            .to_string()
            .split(';')
            .next()
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .to_string(),
        None => "application/json".to_string(),
    };
    if first_accept == "text/event-stream" {
        Ok(tide::sse::upgrade(
            req,
            move |req: Request<(Arc<Session>, String)>, sender: Sender| async move {
                let key_expr = path_to_key_expr(req.url().path(), &req.state().1).to_owned();
                async_std::task::spawn(async move {
                    log::debug!(
                        "Subscribe to {} for SSE stream (task {})",
                        key_expr,
                        async_std::task::current().id()
                    );
                    let sender = &sender;
                    let mut sub = req.state().0.subscribe(&key_expr).await.unwrap();
                    loop {
                        let sample = sub.receiver().next().await.unwrap();
                        let send = async {
                            if let Err(e) = sender
                                .send(&sample.kind.to_string(), sample_to_json(sample), None)
                                .await
                            {
                                log::warn!("Error sending data from the SSE stream: {}", e);
                            }
                            true
                        };
                        let wait = async {
                            async_std::task::sleep(std::time::Duration::new(10, 0)).await;
                            false
                        };
                        if !async_std::prelude::FutureExt::race(send, wait).await {
                            log::debug!(
                                "SSE timeout! Unsubscribe and terminate (task {})",
                                async_std::task::current().id()
                            );
                            if let Err(e) = sub.close().await {
                                log::error!("Error undeclaring subscriber: {}", e);
                            }
                            break;
                        }
                    }
                });
                Ok(())
            },
        ))
    } else {
        let url = req.url();
        let key_expr = path_to_key_expr(url.path(), &req.state().1);
        let query_part = url.query().map(|q| format!("?{}", q));
        let selector = if let Some(q) = &query_part {
            Selector::from(key_expr).with_value_selector(q)
        } else {
            key_expr.into()
        };
        let consolidation = if selector.has_time_range() {
            QueryConsolidation::none()
        } else {
            QueryConsolidation::default()
        };
        match req
            .state()
            .0
            .get(&selector)
            .consolidation(consolidation)
            .await
        {
            Ok(receiver) => {
                if first_accept == "text/html" {
                    Ok(response(
                        StatusCode::Ok,
                        Mime::from_str("text/html").unwrap(),
                        &to_html(receiver).await,
                    ))
                } else {
                    Ok(response(
                        StatusCode::Ok,
                        Mime::from_str("application/json").unwrap(),
                        &to_json(receiver).await,
                    ))
                }
            }
            Err(e) => Ok(response(
                StatusCode::InternalServerError,
                Mime::from_str("text/plain").unwrap(),
                &e.to_string(),
            )),
        }
    }
}

async fn write(mut req: Request<(Arc<Session>, String)>) -> tide::Result<Response> {
    log::trace!("Incoming PUT request: {:?}", req);
    match req.body_bytes().await {
        Ok(bytes) => {
            let key_expr = path_to_key_expr(req.url().path(), &req.state().1);
            let encoding: Encoding = req.content_type().map(|m| m.into()).unwrap_or_default();

            // @TODO: Define the right congestion control value
            match req
                .state()
                .0
                .put(&key_expr, bytes)
                .encoding(encoding)
                .kind(method_to_kind(req.method()))
                .await
            {
                Ok(_) => Ok(Response::new(StatusCode::Ok)),
                Err(e) => Ok(response(
                    StatusCode::InternalServerError,
                    Mime::from_str("text/plain").unwrap(),
                    &e.to_string(),
                )),
            }
        }
        Err(e) => Ok(response(
            StatusCode::NoContent,
            Mime::from_str("text/plain").unwrap(),
            &e.to_string(),
        )),
    }
}

pub async fn run(runtime: Runtime, port: String) {
    // Try to initiate login.
    // Required in case of dynamic lib, otherwise no logs.
    // But cannot be done twice in case of static link.
    let _ = env_logger::try_init();

    let http_port = parse_http_port(&port);

    let pid = runtime.get_pid_str();
    let session = Session::init(runtime, true, vec![], vec![]).await;

    let mut app = Server::with_state((Arc::new(session), pid));
    app.with(
        tide::security::CorsMiddleware::new()
            .allow_methods(
                "GET, PUT, PATCH, DELETE"
                    .parse::<http_types::headers::HeaderValue>()
                    .unwrap(),
            )
            .allow_origin(tide::security::Origin::from("*"))
            .allow_credentials(false),
    );

    app.at("/").get(query).put(write).patch(write).delete(write);
    app.at("*").get(query).put(write).patch(write).delete(write);

    if let Err(e) = app.listen(http_port).await {
        log::error!("Unable to start http server for REST : {:?}", e);
    }
}

fn path_to_key_expr<'a>(path: &'a str, pid: &str) -> KeyExpr<'a> {
    if path == "/@/router/local" {
        KeyExpr::from(format!("/@/router/{}", pid))
    } else if let Some(suffix) = path.strip_prefix("/@/router/local/") {
        KeyExpr::from(format!("/@/router/{}/{}", pid, suffix))
    } else {
        KeyExpr::from(path)
    }
}