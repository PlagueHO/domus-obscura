//! Isolated browser test harness, not an authentication mode of the application.
//! Only generated fixture roots and a fresh temporary database are accessible.
use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use domus::application::{add_location, scan_location};
use domus::domain::{EditionSelection, LibraryPath, OutputSettings, Page};
use domus::infrastructure::store::{
    Entry, JobEvent, JobSummary, Location, PlanRequest, SettingsVersion, Store, StoredPlan,
};
use domus::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

const AUTHORITY: &str = "127.0.0.1:4173";
const ORIGIN: &str = "http://127.0.0.1:4173";

#[path = "local_test/album.rs"]
mod album;

#[path = "local_test/library.rs"]
mod library;

struct Fixture {
    store: Store,
    location: i64,
    job: i64,
    album: Option<album::AlbumTest>,
    _directory: TempDir,
}

async fn album_status(State(shared): State<Shared>) -> ApiResult<Json<Option<album::AlbumReport>>> {
    database(shared, |fixture| {
        Ok(fixture.album.as_ref().map(|album| album.report.clone()))
    })
    .await
    .map(Json)
}

async fn album_run(State(shared): State<Shared>) -> ApiResult<Json<album::AlbumReport>> {
    static RUN: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let _permit = RUN
        .try_acquire()
        .map_err(|_| Failure(Error::Conflict("an album test is already running".into())))?;
    let result = album::run().await?;
    database(shared, move |fixture| {
        let report = result.report.clone();
        fixture.album = Some(result);
        Ok(report)
    })
    .await
    .map(Json)
}

async fn album_audio(
    State(shared): State<Shared>,
    axum::extract::Path(track): axum::extract::Path<usize>,
) -> ApiResult<Response> {
    database(shared, move |fixture| {
        let bytes = fixture
            .album
            .as_ref()
            .and_then(|album| {
                track
                    .checked_sub(1)
                    .and_then(|index| album.audio.get(index))
            })
            .ok_or_else(|| {
                Error::InvalidInput("run the sample album test and select a generated track".into())
            })?;
        Ok(([(header::CONTENT_TYPE, "audio/flac")], bytes.clone()).into_response())
    })
    .await
}

impl Fixture {
    fn new() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let media = directory.path().join("sample-library");
        let album = media.join("S").join("SAMPLE ARTIST").join("SAMPLE ALBUM");
        std::fs::create_dir_all(&album)?;
        std::fs::write(
            album.join("README.txt"),
            b"Generated browsing fixture. This is not an audio file.\n",
        )?;
        library::generate(&album)?;
        let alternate = media.join("Other folder");
        std::fs::create_dir(&alternate)?;
        library::generate(&alternate)?;
        let mut store = Store::open(&directory.path().join("state.db"))?;
        let location = add_location(&store, &media, "main")?;
        scan_location(&mut store, location)?;
        store.save_settings(None, &OutputSettings::default())?;
        let job = store.enqueue(1)?;
        Ok(Self {
            store,
            location,
            job,
            album: None,
            _directory: directory,
        })
    }

    fn snapshot(&self) -> Result<Snapshot> {
        let jobs = self.store.jobs(Page::new(0, 10)?)?;
        let current = jobs
            .iter()
            .find(|job| job.id == self.job)
            .ok_or(Error::Internal("fixture job missing"))?;
        let plan = if current.plan_version > 1 {
            Some(self.store.plan(self.job, current.plan_version)?)
        } else {
            None
        };
        Ok(Snapshot {
            job_id: self.job,
            plan_version: current.plan_version,
            location: self.store.location(self.location)?,
            settings: self
                .store
                .settings()?
                .ok_or(Error::Internal("fixture settings missing"))?,
            jobs,
            events: self.store.events(Page::new(0, 200)?)?,
            plan,
        })
    }
}

#[derive(Serialize)]
struct Snapshot {
    job_id: i64,
    plan_version: i64,
    location: Location,
    settings: SettingsVersion,
    jobs: Vec<JobSummary>,
    events: Vec<JobEvent>,
    plan: Option<StoredPlan>,
}

type Shared = Arc<Mutex<Fixture>>;

#[derive(Debug)]
struct Failure(Error);
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self(error)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::InvalidInput(_) | Error::Json(_) => StatusCode::BAD_REQUEST,
            Error::Conflict(_) => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        eprintln!("local test: {}", self.0);
        (
            status,
            Json(serde_json::json!({"error": self.0.to_string()})),
        )
            .into_response()
    }
}
type ApiResult<T> = std::result::Result<T, Failure>;

async fn database<T, F>(shared: Shared, operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Fixture) -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut fixture = shared
            .lock()
            .map_err(|_| Error::Internal("fixture lock failed"))?;
        operation(&mut fixture)
    })
    .await
    .map_err(|_| Failure(Error::Internal("fixture operation failed")))?
    .map_err(Failure::from)
}

async fn snapshot(State(shared): State<Shared>) -> ApiResult<Json<Snapshot>> {
    database(shared, |fixture| fixture.snapshot())
        .await
        .map(Json)
}

async fn prepare(
    State(shared): State<Shared>,
    Json(request): Json<PlanRequest>,
) -> ApiResult<Json<StoredPlan>> {
    database(shared, move |fixture| {
        if request.job_id != fixture.job || request.locations != [fixture.location] {
            return Err(Error::InvalidInput(
                "only the generated fixture job and location are allowed".into(),
            ));
        }
        fixture.store.prepare_plan(request)
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    version: i64,
}

async fn approve(
    State(shared): State<Shared>,
    Json(request): Json<Approval>,
) -> ApiResult<Json<Snapshot>> {
    database(shared, move |fixture| {
        fixture.store.approve_plan(fixture.job, request.version)?;
        fixture.snapshot()
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveSettings {
    expected_version: Option<i64>,
    settings: OutputSettings,
}

async fn save_settings(
    State(shared): State<Shared>,
    Json(request): Json<SaveSettings>,
) -> ApiResult<Json<Snapshot>> {
    database(shared, move |fixture| {
        fixture
            .store
            .save_settings(request.expected_version, &request.settings)?;
        fixture.snapshot()
    })
    .await
    .map(Json)
}

async fn scan(State(shared): State<Shared>) -> ApiResult<Json<Snapshot>> {
    database(shared, |fixture| {
        scan_location(&mut fixture.store, fixture.location)?;
        fixture.snapshot()
    })
    .await
    .map(Json)
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Browse {
    #[serde(default)]
    folder: String,
    #[serde(default)]
    after: i64,
}
async fn browse(
    State(shared): State<Shared>,
    Query(query): Query<Browse>,
) -> ApiResult<Json<Vec<Entry>>> {
    database(shared, move |fixture| {
        fixture.store.browse(
            fixture.location,
            &LibraryPath::parse(&query.folder)?,
            Page::new(query.after, 25)?,
        )
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    edition: EditionSelection,
    settings: OutputSettings,
}
async fn preview(
    Json(request): Json<Preview>,
) -> ApiResult<Json<Vec<domus::domain::OutputPreview>>> {
    tokio::task::spawn_blocking(move || request.edition.preview(&request.settings))
        .await
        .map_err(|_| Failure(Error::Internal("preview operation failed")))?
        .map(Json)
        .map_err(Failure::from)
}

async fn guard(request: Request, next: Next) -> Response {
    let headers = request.headers();
    let unique = |name: header::HeaderName, expected: &str| {
        headers.get_all(&name).iter().count() == 1
            && headers.get(&name).and_then(|value| value.to_str().ok()) == Some(expected)
    };
    let safe = unique(header::HOST, AUTHORITY)
        && headers
            .get_all(header::ORIGIN)
            .iter()
            .all(|value| value == ORIGIN)
        && !headers
            .get_all("sec-fetch-site")
            .iter()
            .any(|value| value == "cross-site")
        && (matches!(*request.method(), Method::GET | Method::HEAD)
            || (unique(header::ORIGIN, ORIGIN)
                && headers.get_all("x-domus-csrf").iter().count() == 1
                && headers
                    .get("x-domus-csrf")
                    .is_some_and(|value| value == "1")));
    let mut response = if safe {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error":"local test origin or host rejected"})),
        )
            .into_response()
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::CONTENT_SECURITY_POLICY, header::HeaderValue::from_static(
        "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; media-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"
    ));
    headers.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    response
}

fn router(fixture: Fixture, index: String, script: Vec<u8>, style: Vec<u8>) -> Router {
    Router::new()
        .route(
            "/assets/local-test.ttf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "font/ttf")],
                    include_bytes!("../../../frontend/app/assets/BebasNeue-Regular.ttf").as_slice(),
                )
            }),
        )
        .route("/", get(move || async { axum::response::Html(index) }))
        .route(
            "/assets/local-test.js",
            get(move || async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    script,
                )
            }),
        )
        .route(
            "/assets/local-test.css",
            get(move || async { ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], style) }),
        )
        .route(
            "/api/locations/{location}/library/matches",
            post(library::matches),
        )
        .route(
            "/api/locations/{location}/library/refresh",
            post(library::refresh),
        )
        .route(
            "/api/locations/{location}/library/{*tail}",
            get(library::read),
        )
        .route("/api/locations/{location}/entries", get(library::entries))
        .route(
            "/api/locations/{location}/availability",
            get(library::availability),
        )
        .route("/local/snapshot", get(snapshot))
        .route("/local/album-test", get(album_status).post(album_run))
        .route("/local/album-test/audio/{track}", get(album_audio))
        .route("/local/entries", get(browse))
        .route("/local/preview", post(preview))
        .route("/local/plan", post(prepare))
        .route("/local/approve", post(approve))
        .route("/local/settings", put(save_settings))
        .route("/local/scan", post(scan))
        .with_state(Arc::new(Mutex::new(fixture)))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn(guard))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    if std::env::args().len() != 1 {
        return Err(
            "The local test harness accepts no database, root or listener arguments.".into(),
        );
    }
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend/dist/local-test");
    let index = std::fs::read_to_string(assets.join("index.html")).map_err(|error| {
        format!("Build local test assets with the local-test task first: {error}")
    })?;
    let script = std::fs::read(assets.join("assets/local-test.js"))?;
    let style = std::fs::read(assets.join("assets/local-test.css"))?;
    let fixture = Fixture::new()?;
    let listener = tokio::net::TcpListener::bind(AUTHORITY).await?;
    println!("Local test harness: {ORIGIN}");
    println!("Generated sample data only. Downloads use loopback HTTP, not Soulseek.");
    println!("No Entra, CAA, live providers or existing-library access.");
    println!("Temporary data resets when this process stops. Press Ctrl+C to stop.");
    axum::serve(listener, router(fixture, index, script, style))
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("local test shutdown signal failed: {error}");
            }
        })
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "local_test/tests.rs"]
mod tests;
