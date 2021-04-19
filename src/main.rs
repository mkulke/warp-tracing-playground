use std::error::Error;
use tracing_bunyan_formatter as bunyan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let tracer = observability::init_tracer()?;
    tracing_subscriber::Registry::default()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(bunyan::JsonStorageLayer)
        .with(bunyan::BunyanFormattingLayer::new(
            env!("CARGO_PKG_NAME").into(),
            std::io::stdout,
        ))
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .init();
    let metrics_exporter = observability::init_metrics_exporter()?;

    let state = models::init_state();
    let api = filters::users(state, metrics_exporter);
    warp::serve(api).run(([127, 0, 0, 1], 3030)).await;

    Ok(())
}

mod filters {
    use super::handlers;
    use super::models::{State, User};
    use super::observability::{record_metrics, MetricsExporter};
    use std::convert::Infallible;
    use warp::Filter;

    pub fn users(
        state: State,
        metrics_exporter: impl MetricsExporter,
    ) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
        list_users(state.clone())
            .or(create_user(state))
            .or(metrics(metrics_exporter))
            .with(warp::trace::request())
            .with(warp::log::custom(record_metrics))
    }

    pub fn list_users(
        state: State,
    ) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
        warp::path!("users")
            .and(warp::get())
            .and(with_state(state))
            .and_then(handlers::list_users)
    }

    pub fn create_user(
        state: State,
    ) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
        warp::path!("users")
            .and(warp::post())
            .and(json_body())
            .and(with_state(state))
            .and_then(handlers::create_user)
    }

    pub fn metrics(
        exporter: impl MetricsExporter,
    ) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
        warp::path!("metrics")
            .and(warp::get())
            .and(with_exporter(exporter))
            .and_then(handlers::metrics)
    }

    fn with_state(state: State) -> impl Filter<Extract = (State,), Error = Infallible> + Clone {
        warp::any().map(move || state.clone())
    }

    fn with_exporter(
        exporter: impl MetricsExporter,
    ) -> impl Filter<Extract = (impl MetricsExporter,), Error = Infallible> + Clone {
        warp::any().map(move || exporter.clone())
    }

    fn json_body() -> impl Filter<Extract = (User,), Error = warp::Rejection> + Clone {
        warp::body::content_length_limit(1024 * 16).and(warp::body::json())
    }
}

mod handlers {
    use super::models::{State, User};
    use super::observability::MetricsExporter;
    use std::convert::Infallible;
    use tracing::instrument;
    use warp::http::StatusCode;

    #[instrument(skip(state))]
    pub async fn list_users(state: State) -> Result<impl warp::Reply, Infallible> {
        let users = state.lock().await.clone();
        Ok(warp::reply::json(&users))
    }

    #[instrument(skip(state))]
    pub async fn create_user(user: User, state: State) -> Result<impl warp::Reply, Infallible> {
        let mut users = state.lock().await;
        users.push(user);
        Ok(StatusCode::CREATED)
    }

    pub async fn metrics(exporter: impl MetricsExporter) -> Result<impl warp::Reply, Infallible> {
        let buf = exporter.export();
        Ok(buf)
    }
}

mod models {
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    pub type State = Arc<Mutex<Vec<User>>>;

    pub fn init_state() -> State {
        Arc::new(Mutex::new(Vec::new()))
    }

    #[derive(Debug, Deserialize, Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    pub enum Gender {
        Female,
        Male,
        Unspecified,
    }

    #[derive(Debug, Deserialize, Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    pub struct User {
        pub id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub first_name: Option<String>,
        pub last_name: String,
        pub gender: Gender,
    }
}

mod observability {
    use lazy_static::lazy_static;
    use opentelemetry::metrics::MetricsError;
    use opentelemetry::metrics::{Counter, ValueRecorder};
    use opentelemetry::sdk;
    use opentelemetry::trace::TraceError;
    use opentelemetry::KeyValue;
    use opentelemetry::{global, Unit};
    use opentelemetry_prometheus::PrometheusExporter;
    use prometheus::Encoder;
    use std::convert::{TryFrom, TryInto};
    use warp::log::Info;

    struct Meters {
        pub incoming_requests: Counter<u64>,
        pub duration: ValueRecorder<u64>,
        pub status_codes: Counter<u64>,
    }

    lazy_static! {
        static ref METERS: Meters = {
            let meter = global::meter("web-service");
            let incoming_requests = meter.u64_counter("incoming_requests").init();
            let duration = meter
                .u64_value_recorder("http.server.duration")
                .with_unit(Unit::new("milliseconds"))
                .init();
            let status_codes = meter.u64_counter("status_codes").init();
            Meters {
                incoming_requests,
                duration,
                status_codes,
            }
        };
    }

    pub fn init_metrics_exporter() -> Result<PrometheusExporter, MetricsError> {
        opentelemetry_prometheus::exporter()
            .with_default_histogram_boundaries(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1., 2.5, 5., 10.,
            ])
            .try_init()
    }

    pub fn init_tracer() -> Result<sdk::trace::Tracer, TraceError> {
        opentelemetry_jaeger::new_pipeline()
            .with_service_name(env!("CARGO_PKG_NAME"))
            .install_batch(opentelemetry::runtime::Tokio)
    }

    pub struct ServiceMetrics {
        pub duration_ms: u64,
        pub status_family: &'static str,
        pub method: &'static str,
        pub path: &'static str,
    }

    impl ServiceMetrics {
        pub fn duration_labels(&self) -> Vec<KeyValue> {
            [
                KeyValue::new("http.status_code", self.status_family),
                KeyValue::new("http.method", self.method),
                KeyValue::new("http.target", self.path),
            ]
            .into()
        }

        pub fn status_code_labels(&self) -> Vec<KeyValue> {
            [
                KeyValue::new("http.status_code", self.status_family),
                KeyValue::new("http.method", self.method),
            ]
            .into()
        }
    }

    impl ServiceMetrics {
        fn record(&self) {
            METERS.incoming_requests.add(1, &[]);
            METERS
                .duration
                .record(self.duration_ms, &self.duration_labels());
            METERS.status_codes.add(1, &self.status_code_labels());
        }
    }

    impl TryFrom<&Info<'_>> for ServiceMetrics {
        type Error = &'static str;

        fn try_from(info: &Info) -> Result<Self, Self::Error> {
            let duration_ms = info.elapsed().as_millis() as u64;
            let status_family = match info.status().as_u16() {
                500..=599 => Ok("500"),
                400..=499 => Ok("400"),
                300..=399 => Ok("300"),
                200..=299 => Ok("200"),
                100..=199 => Ok("100"),
                _ => Err("unknown status code"),
            }?;
            let method = match info.method().as_str() {
                "GET" => Ok("GET"),
                "POST" => Ok("POST"),
                _ => Err("unknown http method"),
            }?;
            let path = match info.path() {
                "/users" => "/users",
                _ => "invalid",
            };
            let metrics = Self {
                duration_ms,
                status_family,
                method,
                path,
            };

            Ok(metrics)
        }
    }

    pub trait MetricsExporter: Clone + Send {
        fn export(&self) -> Vec<u8>;
    }

    impl MetricsExporter for PrometheusExporter {
        fn export(&self) -> Vec<u8> {
            let encoder = prometheus::TextEncoder::new();
            let metric_families = self.registry().gather();
            let mut buf = Vec::new();
            encoder.encode(&metric_families, &mut buf).unwrap();
            buf
        }
    }

    pub fn record_metrics(info: Info) {
        if info.path() == "/metrics" {
            return;
        }

        let result: Result<ServiceMetrics, _> = (&info).try_into();
        if let Ok(metrics) = result {
            metrics.record();
        };
    }
}

#[cfg(test)]
mod tests {
    use super::filters;
    use super::models::init_state;
    use super::observability::init_metrics_exporter;
    use warp::http::StatusCode;
    use warp::test::request;

    #[tokio::test]
    async fn get_users() {
        let api = filters::users(init_state(), init_metrics_exporter().unwrap());

        let response = request().method("GET").path("/users").reply(&api).await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_user() {
        let state = init_state();
        let api = filters::users(state.clone(), init_metrics_exporter().unwrap());

        let response = request()
            .method("POST")
            .path("/users")
            .body(
                r#"{
                    "firstName": "Jane",
                    "lastName": "Doe",
                    "gender": "female",
                    "id": 123
                }"#,
            )
            .reply(&api)
            .await;

        assert_eq!(response.status(), StatusCode::CREATED);
        let users = state.lock().await;
        assert_eq!(users[0].id, 123);
    }
}
