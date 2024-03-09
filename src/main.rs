use clap::Parser;
use env_logger::Env;
use std::collections::HashMap;
use std::convert::Infallible;
use std::fs::read_to_string;
use std::io::SeekFrom;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{header, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::{mpsc, Mutex};
use tokio::time::interval;

static DEFAULT_TEMPLATE: &str = include_str!("../example.html");

/// An iframe-based website traffic counter / server, written in Rust.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The address the server will bind to.
    #[arg(long, default_value_t = String::from("127.0.0.1:32069"))]
    ip: String,

    /// The path to the HTML template used when serving the iframe.
    /// See https://github.com/msparkles/iframe-traffic-counter/blob/main/example.html for example file.
    #[arg()]
    template: Option<PathBuf>,

    /// Color of the text, in CSS color.
    #[arg(long, default_value_t = String::from("white"))]
    color: String,

    /// Path to the visits storage file
    #[arg(long, default_value_t = String::from("visits.txt"))]
    storage: String,
}

async fn handle(
    req: Request<hyper::body::Incoming>,
    template: Arc<str>,
    visits: Arc<Mutex<HashMap<String, usize>>>,
) -> hyper::http::Result<Response<BoxBody<Bytes, Infallible>>> {
    let Some(referer) = req
        .headers()
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
    else {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Empty::default().boxed());
    };

    log::debug!("Accepted referer: {:?}", referer);

    let html = {
        let mut lock = visits.lock().await;
        let visit = lock.entry(referer.to_string()).or_insert(0);
        *visit += 1;
        template.replace("{{VISIT_COUNT}}", visit.to_string().as_str())
    };

    Ok(Response::new(BoxBody::new(html)))
}

fn fill_values(args: &Args, template: &str) -> String {
    template.replace("{{COLOR}}", &args.color)
}

fn write_visits(visits: &HashMap<String, usize>) -> String {
    visits
        .iter()
        .fold(String::new(), |s, (server, v)| format!("{server} {v}\n{s}"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let template: Arc<str>;
    if let Some(path) = args.template.clone() {
        template = Arc::from(fill_values(&args, &read_to_string(path)?));
    } else {
        template = Arc::from(fill_values(&args, DEFAULT_TEMPLATE));
    }

    {
        let mut env = Env::default();
        env = env.default_filter_or("debug");
        env_logger::Builder::from_env(env).init();
    }

    let addr = SocketAddr::from_str(&args.ip)?;

    log::info!("Listening on {addr}");
    let listener = TcpListener::bind(addr).await?;

    let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);
    tokio::spawn(async move {
        if let Ok(()) = signal::ctrl_c().await {
            cancel_tx.send(()).await.unwrap();
        }
    });

    let storage_path = PathBuf::from(args.storage);
    let visits = {
        let read_visits = read_to_string(&storage_path).unwrap_or(String::new());

        let mut visits = HashMap::default();

        for visit in read_visits.lines() {
            let mut split = visit.split(' ');
            if let Some((server, v)) = split.next().zip(split.next()) {
                if let Ok(v) = v.parse::<usize>() {
                    visits.insert(server.to_string(), v);
                }
            }
        }

        Arc::new(Mutex::new(visits))
    };

    let mut storage = tokio::fs::OpenOptions::new()
        .write(true)
        .append(false)
        .create(true)
        .open(&storage_path)
        .await?;

    let mut update_timer = interval(Duration::from_secs(60));

    loop {
        let cancel_rx = &mut cancel_rx;
        let template = template.clone();
        let visits = visits.clone();

        tokio::select! {
            _ = cancel_rx.recv() => {
                log::info!("Shutting down!");
                let visits = visits.lock().await;
                storage.seek(SeekFrom::Start(0)).await?;
                storage.write_all(write_visits(&visits).as_bytes()).await?;
                storage.flush().await?;
                return Ok(());
            }
            _ = update_timer.tick() => {
                log::debug!("Periodically saving visits to {storage_path:?}!");
                let visits = visits.lock().await;
                storage.seek(SeekFrom::Start(0)).await?;
                storage.write_all(write_visits(&visits).as_bytes()).await?;
                storage.flush().await?;
            }
            Ok((stream, _)) = listener.accept() => {
                let io = TokioIo::new(stream);

                tokio::task::spawn(async move {
                    if let Err(err) = http1::Builder::new()
                        .serve_connection(io, service_fn(move |v| handle(v, template.clone(), visits.clone())))
                        .await
                    {
                        log::error!("Error serving connection: {err:?}");
                    }
                });
            }
        }
    }
}
