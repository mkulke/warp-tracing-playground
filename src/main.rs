use std::error::Error;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let state = models::init_state();
    let api = filters::users(state);

    let (tracer, _uninstall) = opentelemetry_jaeger::new_pipeline()
        .with_service_name(env!("CARGO_PKG_NAME"))
        .install()?;
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = Registry::default().with(telemetry);
    tracing::subscriber::set_global_default(subscriber)?;

    warp::serve(api).run(([127, 0, 0, 1], 3030)).await;

    Ok(())
}

mod filters {
    use super::handlers;
    use super::models::{State, User};
    use std::convert::Infallible;
    use warp::Filter;

    pub fn users(
        state: State,
    ) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
        list_users(state.clone())
            .or(create_user(state.clone()))
            .with(warp::trace::request())
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

    fn with_state(state: State) -> impl Filter<Extract = (State,), Error = Infallible> + Clone {
        warp::any().map(move || state.clone())
    }

    fn json_body() -> impl Filter<Extract = (User,), Error = warp::Rejection> + Clone {
        warp::body::content_length_limit(1024 * 16).and(warp::body::json())
    }
}

mod handlers {
    use super::models::{State, User};
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
