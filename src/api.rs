use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use std::str::FromStr;

use crate::cache::{CacheStats, SharedCache};
use crate::models::{CreateRecord, HealthResponse, Record, RecordType};
use crate::store::SharedStore;

/// Combined application state for the API router.
#[derive(Clone)]
pub struct AppState {
    pub store: SharedStore,
    pub cache: SharedCache,
}

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
    State(state): State<AppState>,
    Json(req): Json<CreateRecord>,
) -> (StatusCode, Json<Record>) {
    let record = state.store.add(req);
    (StatusCode::CREATED, Json(record))
}

async fn list_records(
    State(state): State<AppState>,
    Query(filter): Query<RecordFilter>,
) -> Result<Json<Vec<Record>>, (StatusCode, String)> {
    let rtype = parse_filter_type(&filter)?;
    Ok(Json(
        state.store.list_filtered(filter.name.as_deref(), rtype),
    ))
}

async fn get_record(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Record>, (StatusCode, String)> {
    state
        .store
        .get(&id)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "record not found".into()))
}

async fn delete_record(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
    if state.store.delete(&id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn delete_records(
    State(state): State<AppState>,
    Query(filter): Query<RecordFilter>,
) -> Result<StatusCode, (StatusCode, String)> {
    let rtype = parse_filter_type(&filter)?;
    if filter.name.is_none() && rtype.is_none() {
        state.store.delete_all();
    } else {
        state.store.delete_filtered(filter.name.as_deref(), rtype);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Debug, serde::Serialize)]
struct FlushResponse {
    flushed: usize,
}

#[derive(Debug, Deserialize)]
struct CacheFilter {
    name: Option<String>,
}

async fn flush_cache(
    State(state): State<AppState>,
    Query(filter): Query<CacheFilter>,
) -> Json<FlushResponse> {
    let count = if let Some(name) = &filter.name {
        state.cache.flush_domain(name)
    } else {
        state.cache.flush()
    };
    Json(FlushResponse { flushed: count })
}

async fn cache_stats(State(state): State<AppState>) -> Json<CacheStats> {
    Json(state.cache.stats())
}

pub fn router(state: AppState) -> Router {
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
        .route("/api/v1/cache", get(cache_stats).delete(flush_cache))
        .with_state(state)
}
