pub mod state;

use actix_cors::Cors;
use actix_server::Server;
use actix_web::{http, web, App, HttpResponse, HttpServer};
use std::net::SocketAddr;
use tracing_actix_web::TracingLogger;

use state::ServerState;

const MAX_WORKERS: usize = 2;

pub fn create_web_server(
    state: ServerState,
    actix_server_addr: SocketAddr,
) -> anyhow::Result<Server> {
    let server = HttpServer::new(move || {
        // create cors
        let cors = Cors::default()
            .allow_any_origin()
            .allowed_methods(vec!["GET", "POST"])
            .allowed_headers(vec![http::header::AUTHORIZATION, http::header::ACCEPT])
            .allowed_header(http::header::CONTENT_TYPE)
            .max_age(3600);

        App::new()
            .app_data(web::Data::new(state.clone()))
            .wrap(TracingLogger::default())
            .wrap(cors)
            .service(web::resource("/health").route(web::get().to(
                |state: web::Data<ServerState>| async move {
                    if !state.is_healthy() {
                        return HttpResponse::ServiceUnavailable().body("Service Unavailable");
                    }
                    HttpResponse::Ok().json(state.get_health().await)
                },
            )))
            .service(web::resource("/metrics").route(web::get().to(
                |state: web::Data<ServerState>| async move {
                    HttpResponse::Ok()
                        .content_type("text/plain")
                        .body(state.telemetry.get_metrics().await)
                },
            )))
    })
    .bind(actix_server_addr)?
    .workers(MAX_WORKERS)
    .shutdown_timeout(20)
    .run();

    Ok(server)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use actix_web::{http, test, web, App, HttpResponse};

    use crate::{
        http::state::{HealthResponse, ServerState},
        telemetry::Telemetry,
    };

    #[actix_web::test]
    async fn test_health_check() {
        let telemetry = Telemetry::new().await.unwrap();
        telemetry.start().await.unwrap();
        let state = ServerState::new(telemetry.clone()).await;

        let app = test::init_service(App::new().app_data(web::Data::new(state.clone())).route(
            "/health",
            web::get().to(|state: web::Data<ServerState>| async move {
                if !state.is_healthy() {
                    return HttpResponse::ServiceUnavailable().body("Service Unavailable");
                }
                HttpResponse::Ok().json(state.get_health().await)
            }),
        ))
        .await;

        let uptime = Duration::from_secs(2);
        tokio::time::sleep(uptime).await;

        let req = test::TestRequest::get().uri("/health").to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), http::StatusCode::OK);

        let result: HealthResponse = test::read_body_json(resp).await;
        assert!(result.uptime >= uptime.as_secs());
    }
}
