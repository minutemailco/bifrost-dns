use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use std::str::FromStr;

use crate::models::{CreateRecord, HealthResponse, Record, RecordType};
use crate::store::SharedStore;

#[derive(Debug, Deserialize)]
struct RecordFilter {
    name: Option<String>,
    #[serde(rename = "type")]
    record_type: Option<String>,
}

fn parse_filter_type(filter: &RecordFilter) -> Result<Option<RecordType>, (StatusCode, String)> {
    filter
        .record_type
        .as_deref()
        .map(RecordType::from_str)
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}

async fn create_record(
    State(store): State<SharedStore>,
    Json(req): Json<CreateRecord>,
) -> (StatusCode, Json<Record>) {
    let record = store.add(req);
    (StatusCode::CREATED, Json(record))
}

async fn list_records(
    State(store): State<SharedStore>,
    Query(filter): Query<RecordFilter>,
) -> Result<Json<Vec<Record>>, (StatusCode, String)> {
    let rtype = parse_filter_type(&filter)?;
    Ok(Json(store.list_filtered(filter.name.as_deref(), rtype)))
}

async fn get_record(
    State(store): State<SharedStore>,
    Path(id): Path<String>,
) -> Result<Json<Record>, (StatusCode, String)> {
    store
        .get(&id)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "record not found".into()))
}

async fn delete_record(State(store): State<SharedStore>, Path(id): Path<String>) -> StatusCode {
    if store.delete(&id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn delete_records(
    State(store): State<SharedStore>,
    Query(filter): Query<RecordFilter>,
) -> Result<StatusCode, (StatusCode, String)> {
    let rtype = parse_filter_type(&filter)?;
    if filter.name.is_none() && rtype.is_none() {
        store.delete_all();
    } else {
        store.delete_filtered(filter.name.as_deref(), rtype);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

pub fn router(store: SharedStore) -> Router {
    Router::new()
        .route("/health", get(health))
        .route(
            "/api/v1/records",
            post(create_record).get(list_records).delete(delete_records),
        )
        .route(
            "/api/v1/records/{id}",
            get(get_record).delete(delete_record),
        )
        .with_state(store)
}
