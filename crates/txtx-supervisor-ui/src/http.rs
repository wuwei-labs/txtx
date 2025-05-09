use super::ASSETS;
use actix_cors::Cors;
use actix_web::dev::ServerHandle;
use actix_web::http::header::{self};
use actix_web::web::{self, Data};
use actix_web::Error;
use actix_web::Responder;
use actix_web::{middleware, App, HttpRequest, HttpResponse, HttpServer};
use juniper_actix::{graphiql_handler, graphql_handler, playground_handler, subscriptions};
use juniper_graphql_ws::ConnectionConfig;
use std::error::Error as StdError;
use std::time::Duration;
use txtx_core::kit::types::frontend::{ClientType, DiscoveryResponse};
use txtx_gql::Context as GraphContext;
use txtx_gql::{new_graphql_schema, GraphqlSchema};

use super::cloud_relayer::{delete_channel, get_channel, open_channel, RelayerContext};

pub async fn start_server(
    gql_context: GraphContext,
    relayer_context: RelayerContext,
    network_binding: &str,
) -> Result<ServerHandle, Box<dyn StdError>> {
    let gql_context = Data::new(gql_context);
    let relayer_context = Data::new(relayer_context);

    let server = HttpServer::new(move || {
        App::new()
            .app_data(Data::new(new_graphql_schema()))
            .app_data(gql_context.clone())
            .app_data(relayer_context.clone())
            .wrap(
                Cors::default()
                    .allow_any_origin()
                    .allowed_methods(vec!["POST", "GET", "OPTIONS", "DELETE"])
                    .allowed_headers(vec![header::AUTHORIZATION, header::ACCEPT])
                    .allowed_header(header::CONTENT_TYPE)
                    .supports_credentials()
                    .max_age(3600),
            )
            .wrap(middleware::Compress::default())
            .wrap(middleware::Logger::default())
            .service(
                web::scope("/api/v1")
                    .route("/channels", web::post().to(open_channel))
                    .route("/channels", web::delete().to(delete_channel))
                    .route("/channels", web::get().to(get_channel))
                    .route("/discovery", web::get().to(discovery)),
            )
            .service(
                web::scope("/gql/v1")
                    .route("/graphql?<request..>", web::get().to(get_graphql))
                    .route("/graphql", web::post().to(post_graphql))
                    .route("/subscriptions", web::get().to(subscriptions)),
            )
            .service(web::resource("/playground").route(web::get().to(playground)))
            .service(web::resource("/graphiql").route(web::get().to(graphiql)))
            .service(dist)
    })
    .workers(5)
    .bind(network_binding)?
    .run();
    let handle = server.handle();
    tokio::spawn(server);

    Ok(handle)
}

async fn playground() -> Result<HttpResponse, Error> {
    playground_handler("/graphql", Some("/subscriptions")).await
}

async fn graphiql() -> Result<HttpResponse, Error> {
    graphiql_handler("/graphql", Some("/subscriptions")).await
}

fn handle_embedded_file(path: &str) -> HttpResponse {
    ASSETS.get_file(path).map_or_else(
        || {
            ASSETS
                .get_file("index.html")
                .map_or_else(|| HttpResponse::NotFound().body("404 Not Found"), parse_file)
        },
        parse_file,
    )
}

fn parse_file(file: &include_dir::File<'_>) -> HttpResponse {
    use mime_guess::from_path;
    file.contents_utf8().map_or_else(
        || {
            HttpResponse::Ok()
                .content_type(from_path(file.path()).first_or_octet_stream().as_ref())
                .body(file.contents().to_owned())
        },
        |content_str| {
            HttpResponse::Ok()
                .content_type(from_path(file.path()).first_or_octet_stream().as_ref())
                .body(content_str.to_owned())
        },
    )
}

#[actix_web::get("/{_:.*}")]
async fn dist(path: web::Path<String>) -> impl Responder {
    let path_str = match path.as_str() {
        "" => "index.html",
        other => other,
    };
    handle_embedded_file(path_str)
}

async fn discovery() -> impl Responder {
    HttpResponse::Ok()
        .json(DiscoveryResponse { needs_credentials: false, client_type: ClientType::Operator })
}

async fn post_graphql(
    req: HttpRequest,
    payload: web::Payload,
    schema: Data<GraphqlSchema>,
    context: Data<GraphContext>,
) -> Result<HttpResponse, Error> {
    graphql_handler(&schema, &context, req, payload).await
}

async fn get_graphql(
    req: HttpRequest,
    payload: web::Payload,
    schema: Data<GraphqlSchema>,
    context: Data<GraphContext>,
) -> Result<HttpResponse, Error> {
    graphql_handler(&schema, &context, req, payload).await
}

async fn subscriptions(
    req: HttpRequest,
    stream: web::Payload,
    schema: Data<GraphqlSchema>,
    context: Data<GraphContext>,
) -> Result<HttpResponse, Error> {
    let ctx = GraphContext {
        protocol_name: context.protocol_name.clone(),
        runbook_name: context.runbook_name.clone(),
        registered_addons: context.registered_addons.clone(),
        runbook_description: context.runbook_description.clone(),
        block_store: context.block_store.clone(),
        block_broadcaster: context.block_broadcaster.clone(),
        action_item_events_tx: context.action_item_events_tx.clone(),
    };
    let config = ConnectionConfig::new(ctx);
    let config = config.with_keep_alive_interval(Duration::from_secs(15));
    subscriptions::ws_handler(req, stream, schema.into_inner(), config).await
}
