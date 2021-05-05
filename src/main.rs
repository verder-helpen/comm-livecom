use jwt::FromJws;
use rocket::{fairing::AdHoc, get, launch, post, routes, State};
use rocket_contrib::{database, databases::postgres, json::Json};
use types::{AuthResultSet, GuestToken, HostToken};

use crate::{config::Config, error::Error, session::Session, util::random_string};
use id_contact_proto::{ClientUrlResponse, SessionOptions, StartRequestAuthOnly};

mod config;
mod error;
mod jwt;
mod session;
mod types;
mod util;

#[database("session")]
pub struct SessionDBConn(postgres::Client);

#[get("/session_options/<guest_token>")]
async fn session_options(
    guest_token: String,
    config: State<'_, Config>,
) -> Result<Json<SessionOptions>, Error> {
    let guest_token = GuestToken::from_jws(&guest_token, config.guest_validator())?;

    let session_options = reqwest::get(format!(
        "{}/session_options/{}",
        config.core_url(),
        &guest_token.purpose
    ))
    .await?
    .text()
    .await?;

    let session_options = serde_json::from_str::<SessionOptions>(&session_options)?;
    Ok(Json(session_options))
}

#[get("/start/<auth_method>/<guest_token>")]
async fn start(
    auth_method: String,
    guest_token: String,
    config: State<'_, Config>,
    db: SessionDBConn,
) -> Result<Json<ClientUrlResponse>, Error> {
    let guest_token = GuestToken::from_jws(&guest_token, config.validator())?;

    let attr_id = random_string(64);
    let purpose = guest_token.purpose.clone();
    let comm_url = guest_token.redirect_url.clone();
    let attr_url = format!("{}/auth_result/{}", config.internal_url(), attr_id);

    let session = Session::new(guest_token, attr_id);
    session.persist(&db).await?;

    let start_request = StartRequestAuthOnly {
        purpose,
        auth_method,
        comm_url,
        attr_url: Some(attr_url),
    };

    let client = reqwest::Client::new();
    let client_url_response = client
        .post(format!("{}/start", config.core_url()))
        .json(&start_request)
        .send()
        .await?
        .text()
        .await?;

    let client_url_response = serde_json::from_str::<ClientUrlResponse>(&client_url_response)?;
    Ok(Json(client_url_response))
}

#[post("/auth_result/<attr_id>", data = "<auth_result>")]
async fn auth_result(
    attr_id: String,
    auth_result: String,
    config: State<'_, Config>,
    db: SessionDBConn,
) -> Result<(), Error> {
    id_contact_jwt::decrypt_and_verify_auth_result(
        &auth_result,
        config.validator(),
        config.decrypter(),
    )?;
    Session::register_auth_result(attr_id, auth_result, &db).await
}

#[get("/session_info/<host_token>")]
async fn session_info(
    host_token: String,
    config: State<'_, Config>,
    db: SessionDBConn,
) -> Result<Json<AuthResultSet>, Error> {
    let host_token = HostToken::from_jws(&host_token, config.host_validator())?;
    let sessions = Session::find_by_room_id(host_token.room_id, &db).await?;

    let auth_results: AuthResultSet = sessions
        .into_iter()
        .map(|s| {
            (
                s.guest_token.name,
                s.auth_result
                    .map(|r| {
                        id_contact_jwt::decrypt_and_verify_auth_result(
                            &r,
                            config.validator(),
                            config.decrypter(),
                        )
                        .ok()
                    })
                    .flatten(),
            )
        })
        .collect();

    Ok(Json(auth_results))
}

#[launch]
fn rocket() -> _ {
    rocket::build()
        .mount(
            "/",
            routes![session_options, start, auth_result, session_info,],
        )
        .attach(SessionDBConn::fairing())
        .attach(AdHoc::config::<Config>())
}
