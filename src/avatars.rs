use crate::store::{Error, Store};
use crate::{App, authorized};
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

const MAX_UPLOAD: usize = 2 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_DECODED: u64 = 64 * 1024 * 1024;
const SIZE: u32 = 256;

pub fn initialize(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "create table if not exists avatars (
             user text primary key,
             revision integer not null,
             image blob
         );",
    )?;
    Ok(())
}

// Retain a tombstone when removing a picture: a later upload must not reuse an
// earlier revision, even when removal and upload happen within one millisecond.
fn save(store: &Store, user: &str, image: Option<&[u8]>) -> Result<i64, Error> {
    Ok(store.conn.query_row(
        "insert into avatars (user, revision, image) values (?1, 1, ?2)
         on conflict(user) do update set revision=revision+1, image=excluded.image
         returning revision",
        params![user, image],
        |row| row.get(0),
    )?)
}

fn picture(store: &Store, user: &str) -> Result<Option<(i64, Vec<u8>)>, Error> {
    Ok(store
        .conn
        .query_row(
            "select revision, image from avatars where user=?1 and image is not null",
            [user],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

fn normalize(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    if bytes.len() > MAX_UPLOAD {
        return Err("Choose a picture smaller than 2 MiB.");
    }
    let invalid = "Choose a valid JPEG or PNG picture.";
    let oversized = "Choose a picture no larger than 4096 by 4096 pixels.";
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| invalid)?;
    if !matches!(reader.format(), Some(ImageFormat::Jpeg | ImageFormat::Png)) {
        return Err(invalid);
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED);
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder().map_err(|_| invalid)?;
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
        || decoder.total_bytes() > MAX_DECODED
    {
        return Err(oversized);
    }
    limits
        .reserve(decoder.total_bytes())
        .map_err(|_| oversized)?;
    decoder.set_limits(limits).map_err(|_| oversized)?;
    let orientation = decoder.orientation().map_err(|_| invalid)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| invalid)?;
    image.apply_orientation(orientation);
    let edge = image.width().min(image.height());
    let image = image.crop_imm(
        (image.width() - edge) / 2,
        (image.height() - edge) / 2,
        edge,
        edge,
    );
    let image = image
        .resize_exact(SIZE, SIZE, image::imageops::FilterType::Triangle)
        .to_rgba8();
    let mut output = Cursor::new(Vec::new());
    // Construct a fresh pixel image so original EXIF/location and other file
    // metadata cannot be copied into the publicly displayed picture.
    DynamicImage::ImageRgba8(image)
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|_| "The picture could not be processed.")?;
    Ok(output.into_inner())
}

fn failure(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"error": message})),
    )
        .into_response()
}

fn storage_failure(error: Error) -> Response {
    eprintln!("profile picture storage failed: {error}");
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "The profile picture could not be saved or loaded. Please try again.",
    )
}

async fn list(State(app): State<Arc<App>>) -> Response {
    let result = (|| -> Result<BTreeMap<String, String>, Error> {
        let store = app.store.lock().unwrap();
        let mut statement = store
            .conn
            .prepare("select user, revision from avatars where image is not null")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut revisions = BTreeMap::new();
        for row in rows {
            let (user, revision) = row?;
            if app.config.users.contains_key(&user) {
                revisions.insert(user, revision.to_string());
            }
        }
        Ok(revisions)
    })();
    match result {
        Ok(revisions) => (
            [(header::CACHE_CONTROL, "private, no-cache")],
            Json(revisions),
        )
            .into_response(),
        Err(error) => storage_failure(error),
    }
}

async fn read(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !app.config.users.contains_key(&user) {
        return failure(StatusCode::NOT_FOUND, "No profile picture.");
    }
    let result = picture(&app.store.lock().unwrap(), &user);
    let (revision, bytes) = match result {
        Ok(Some(picture)) => picture,
        Ok(None) => return failure(StatusCode::NOT_FOUND, "No profile picture."),
        Err(error) => return storage_failure(error),
    };
    let etag = format!("\"avatar-{revision}\"");
    let unchanged = headers.get_all(header::IF_NONE_MATCH).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate.trim_start_matches("W/") == etag
            })
        })
    });
    let headers = [
        (header::CONTENT_TYPE, "image/png".to_string()),
        (header::ETAG, etag),
        (header::CACHE_CONTROL, "private, no-cache".to_string()),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
    ];
    if unchanged {
        (StatusCode::NOT_MODIFIED, headers).into_response()
    } else {
        (headers, bytes).into_response()
    }
}

async fn upload(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    request: Request,
) -> Response {
    // Authenticate before reading or decoding the body. The explicit byte limit
    // also covers chunked bodies, which have no Content-Length header.
    if !authorized(&app, &user, request.headers()) {
        return failure(StatusCode::UNAUTHORIZED, "Connect your own account first.");
    }
    let bytes = match to_bytes(request.into_body(), MAX_UPLOAD).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return failure(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Choose a picture smaller than 2 MiB.",
            );
        }
    };
    let image = match tokio::task::spawn_blocking(move || normalize(&bytes)).await {
        Ok(Ok(image)) => image,
        Ok(Err(message)) => return failure(StatusCode::BAD_REQUEST, message),
        Err(error) => {
            eprintln!("profile picture processing failed: {error}");
            return failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The picture could not be processed. Please try again.",
            );
        }
    };
    match save(&app.store.lock().unwrap(), &user, Some(&image)) {
        Ok(revision) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({"revision": revision.to_string()})),
        )
            .into_response(),
        Err(error) => storage_failure(error),
    }
}

async fn remove(
    State(app): State<Arc<App>>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&app, &user, &headers) {
        return failure(StatusCode::UNAUTHORIZED, "Connect your own account first.");
    }
    match save(&app.store.lock().unwrap(), &user, None) {
        Ok(_) => (
            StatusCode::NO_CONTENT,
            [(header::CACHE_CONTROL, "no-store")],
        )
            .into_response(),
        Err(error) => storage_failure(error),
    }
}

async fn script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/avatars.js"),
    )
}

async fn style() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/avatars.css"),
    )
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/avatars", get(list))
        .route("/api/avatar/{user}", get(read).post(upload).delete(remove))
        .route("/avatars.js", get(script))
        .route("/avatars.css", get(style))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, game::Week};
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use image::{GenericImageView, Rgb, RgbImage};
    use std::path::PathBuf;
    use std::sync::{Mutex, RwLock};
    use tower::ServiceExt;

    fn fixture() -> (Arc<App>, PathBuf) {
        fixture_with_private_site(false)
    }

    fn fixture_with_private_site(private_site: bool) -> (Arc<App>, PathBuf) {
        let (store, path) = crate::decks::tests::temporary_store();
        let config: Config = serde_json::from_value(serde_json::json!({
            "private_site": private_site,
            "users": {"cerro": {"token": "cerro-secret"}, "hill": {"token": "hill-secret"}}
        }))
        .unwrap();
        (
            Arc::new(App {
                access: crate::access::Access::from_config(&config).unwrap(),
                config,
                week: Week::default(),
                store: Mutex::new(store),
                players: RwLock::new(Default::default()),
            }),
            path,
        )
    }

    fn cleanup(app: Arc<App>, path: PathBuf) {
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }

    fn encoded(image: RgbImage, format: ImageFormat) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut output, format)
            .unwrap();
        output.into_inner()
    }

    fn sample(format: ImageFormat) -> Vec<u8> {
        encoded(RgbImage::from_pixel(12, 8, Rgb([20, 80, 180])), format)
    }

    fn headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    async fn post(app: &Arc<App>, user: &str, token: &str, bytes: Vec<u8>) -> Response {
        let request = Request::builder()
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(bytes))
            .unwrap();
        upload(State(app.clone()), Path(user.into()), request).await
    }

    async fn json(response: Response) -> serde_json::Value {
        serde_json::from_slice(&to_bytes(response.into_body(), MAX_UPLOAD).await.unwrap()).unwrap()
    }

    async fn routed(
        app: &Arc<App>,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .extension(ConnectInfo(
                "192.0.2.1:40000".parse::<std::net::SocketAddr>().unwrap(),
            ));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        crate::router(app.clone())
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn private_site_guards_avatar_metadata_photos_assets_and_conditional_reads() {
        let (app, path) = fixture_with_private_site(true);
        save(
            &app.store.lock().unwrap(),
            "cerro",
            Some(&sample(ImageFormat::Png)),
        )
        .unwrap();
        for endpoint in ["/api/avatars", "/api/avatar/cerro?v=1"] {
            for headers in [Vec::new(), vec![("if-none-match", "\"avatar-1\"")]] {
                let response = routed(&app, "GET", endpoint, &headers, Vec::new()).await;
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{endpoint}");
                assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            }
        }
        for endpoint in ["/avatars.js", "/avatars.css"] {
            assert_eq!(
                routed(&app, "GET", endpoint, &[], Vec::new())
                    .await
                    .status(),
                StatusCode::SEE_OTHER
            );
        }
        let reader = [("authorization", "Bearer hill-secret")];
        assert_eq!(
            json(routed(&app, "GET", "/api/avatars", &reader, Vec::new()).await).await,
            serde_json::json!({"cerro":"1"})
        );
        let response = routed(&app, "GET", "/api/avatar/cerro?v=1", &reader, Vec::new()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        cleanup(app, path);
    }

    #[tokio::test]
    async fn private_browser_sessions_read_avatars_and_only_owners_can_change_them() {
        let (app, path) = fixture_with_private_site(true);
        let photo = sample(ImageFormat::Png);
        save(&app.store.lock().unwrap(), "cerro", Some(&photo)).unwrap();
        let session = routed(
            &app,
            "POST",
            "/auth/session",
            &[("authorization", "Bearer hill-secret")],
            Vec::new(),
        )
        .await;
        assert_eq!(session.status(), StatusCode::NO_CONTENT);
        let cookie = session.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let browser = [("cookie", cookie.as_str())];
        for endpoint in [
            "/api/avatars",
            "/api/avatar/cerro?v=1",
            "/avatars.js",
            "/avatars.css",
        ] {
            let response = routed(&app, "GET", endpoint, &browser, Vec::new()).await;
            assert_eq!(response.status(), StatusCode::OK, "{endpoint}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        for credentials in [
            vec![("cookie", cookie.as_str()), ("x-ankiquest-csrf", "1")],
            vec![("authorization", "Bearer hill-secret")],
        ] {
            for method in ["POST", "DELETE"] {
                assert_eq!(
                    routed(
                        &app,
                        method,
                        "/api/avatar/cerro",
                        &credentials,
                        photo.clone()
                    )
                    .await
                    .status(),
                    StatusCode::UNAUTHORIZED
                );
            }
        }
        for method in ["POST", "DELETE"] {
            assert_eq!(
                routed(&app, method, "/api/avatar/cerro", &browser, photo.clone())
                    .await
                    .status(),
                StatusCode::FORBIDDEN,
                "cookie writes require the CSRF header"
            );
        }
        let owner = [
            ("cookie", cookie.as_str()),
            ("authorization", "Bearer cerro-secret"),
        ];
        assert_eq!(
            json(routed(&app, "POST", "/api/avatar/cerro", &owner, photo).await).await,
            serde_json::json!({"revision":"2"})
        );
        assert_eq!(
            routed(&app, "DELETE", "/api/avatar/cerro", &owner, Vec::new())
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        let logout = [("cookie", cookie.as_str()), ("x-ankiquest-csrf", "1")];
        assert_eq!(
            routed(&app, "POST", "/auth/logout", &logout, Vec::new())
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        for endpoint in ["/api/avatars", "/api/avatar/cerro?v=1"] {
            let response = routed(&app, "GET", endpoint, &browser, Vec::new()).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        cleanup(app, path);
    }

    #[tokio::test]
    async fn a_token_shared_by_two_players_cannot_change_either_profile_picture() {
        let (mut app, path) = fixture();
        Arc::get_mut(&mut app)
            .unwrap()
            .config
            .users
            .get_mut("hill")
            .unwrap()
            .token = Some("cerro-secret".into());
        for user in ["cerro", "hill"] {
            for method in ["POST", "DELETE"] {
                assert_eq!(
                    routed(
                        &app,
                        method,
                        &format!("/api/avatar/{user}"),
                        &[("authorization", "Bearer cerro-secret")],
                        sample(ImageFormat::Png),
                    )
                    .await
                    .status(),
                    StatusCode::UNAUTHORIZED,
                    "{method} {user} must have a unique owner token"
                );
            }
        }
        cleanup(app, path);
    }

    #[tokio::test]
    async fn public_site_keeps_avatar_reads_public_and_honors_image_etags() {
        let (app, path) = fixture();
        save(
            &app.store.lock().unwrap(),
            "cerro",
            Some(&sample(ImageFormat::Png)),
        )
        .unwrap();
        assert_eq!(
            json(routed(&app, "GET", "/api/avatars", &[], Vec::new()).await).await,
            serde_json::json!({"cerro":"1"})
        );
        let image = routed(&app, "GET", "/api/avatar/cerro?v=1", &[], Vec::new()).await;
        assert_eq!(image.status(), StatusCode::OK);
        let etag = image.headers()[header::ETAG].to_str().unwrap().to_string();
        assert_eq!(image.headers()[header::CACHE_CONTROL], "no-store");
        let unchanged = routed(
            &app,
            "GET",
            "/api/avatar/cerro?v=1",
            &[("if-none-match", &etag)],
            Vec::new(),
        )
        .await;
        assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(unchanged.headers()[header::CACHE_CONTROL], "no-store");
        cleanup(app, path);
    }

    #[test]
    fn png_and_jpeg_become_small_square_pngs_with_centered_content() {
        for format in [ImageFormat::Png, ImageFormat::Jpeg] {
            let input = RgbImage::from_fn(12, 8, |x, _| {
                if (2..10).contains(&x) {
                    Rgb([10, 200, 30])
                } else {
                    Rgb([250, 10, 10])
                }
            });
            let output = normalize(&encoded(input, format)).unwrap();
            assert_eq!(image::guess_format(&output).unwrap(), ImageFormat::Png);
            let decoded = image::load_from_memory(&output).unwrap();
            assert_eq!(decoded.dimensions(), (SIZE, SIZE));
            let center = decoded.get_pixel(SIZE / 2, SIZE / 2);
            assert!(center[1] > 150 && center[0] < 60);
            assert!(output.len() < MAX_UPLOAD);
        }
    }

    #[test]
    fn invalid_unsupported_truncated_and_oversized_images_are_rejected() {
        for bytes in [
            Vec::new(),
            b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>".to_vec(),
            b"GIF89a".to_vec(),
            sample(ImageFormat::Png)[..20].to_vec(),
            vec![0; MAX_UPLOAD + 1],
            encoded(RgbImage::new(MAX_DIMENSION + 1, 1), ImageFormat::Png),
            encoded(RgbImage::new(1, MAX_DIMENSION + 1), ImageFormat::Png),
        ] {
            assert!(normalize(&bytes).is_err());
        }
    }

    #[test]
    fn jpeg_orientation_is_applied_and_original_metadata_is_discarded() {
        let image = RgbImage::from_fn(16, 16, |x, _| {
            if x < 8 {
                Rgb([250, 10, 10])
            } else {
                Rgb([10, 10, 250])
            }
        });
        let jpeg = encoded(image, ImageFormat::Jpeg);
        // Little-endian TIFF IFD containing orientation=6 (90 degrees clockwise).
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let private_marker = b"private-location-metadata";
        let mut input = jpeg[..2].to_vec();
        for (marker, payload) in [(0xe1, exif.as_slice()), (0xfe, private_marker.as_slice())] {
            input.extend_from_slice(&[0xff, marker]);
            input.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
            input.extend_from_slice(payload);
        }
        input.extend_from_slice(&jpeg[2..]);
        let output = normalize(&input).unwrap();
        assert!(
            !output
                .windows(private_marker.len())
                .any(|part| part == private_marker)
        );
        assert!(!output.windows(4).any(|part| part == b"eXIf"));
        let decoded = image::load_from_memory(&output).unwrap();
        let top = decoded.get_pixel(SIZE / 2, SIZE / 4);
        let bottom = decoded.get_pixel(SIZE / 2, 3 * SIZE / 4);
        assert!(top[0] > 200 && top[2] < 60, "orientation puts red on top");
        assert!(
            bottom[2] > 200 && bottom[0] < 60,
            "orientation puts blue below"
        );
    }

    #[test]
    fn revisions_and_removal_survive_restart_without_reusing_a_version() {
        let (store, path) = crate::decks::tests::temporary_store();
        let image = normalize(&sample(ImageFormat::Png)).unwrap();
        assert_eq!(save(&store, "cerro", Some(&image)).unwrap(), 1);
        assert_eq!(save(&store, "cerro", Some(&image)).unwrap(), 2);
        assert_eq!(save(&store, "cerro", None).unwrap(), 3);
        assert!(picture(&store, "cerro").unwrap().is_none());
        drop(store);
        let store = Store::open(&path).unwrap();
        assert!(picture(&store, "cerro").unwrap().is_none());
        assert_eq!(save(&store, "cerro", Some(&image)).unwrap(), 4);
        assert_eq!(picture(&store, "cerro").unwrap(), Some((4, image)));
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn only_the_owner_can_upload_or_remove_their_picture() {
        let (app, path) = fixture();
        for token in ["", "hill-secret", "wrong"] {
            let response = post(&app, "cerro", token, sample(ImageFormat::Png)).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(
                remove(State(app.clone()), Path("cerro".into()), headers(token))
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            post(&app, "unknown", "cerro-secret", sample(ImageFormat::Png))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(
            picture(&app.store.lock().unwrap(), "cerro")
                .unwrap()
                .is_none()
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn upload_list_read_etag_and_remove_follow_the_public_contract() {
        let (app, path) = fixture();
        assert_eq!(
            json(list(State(app.clone())).await).await,
            serde_json::json!({})
        );
        assert_eq!(
            json(post(&app, "cerro", "cerro-secret", sample(ImageFormat::Jpeg)).await).await,
            serde_json::json!({"revision":"1"})
        );
        assert_eq!(
            json(list(State(app.clone())).await).await,
            serde_json::json!({"cerro":"1"})
        );
        let response = read(State(app.clone()), Path("cerro".into()), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-cache"
        );
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        let etag = response.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .to_string();
        let bytes = to_bytes(response.into_body(), MAX_UPLOAD).await.unwrap();
        assert_eq!(
            image::load_from_memory(&bytes).unwrap().dimensions(),
            (SIZE, SIZE)
        );
        for condition in [etag.clone(), format!("\"older\", W/{etag}"), "*".into()] {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, condition.parse().unwrap());
            let response = read(State(app.clone()), Path("cerro".into()), headers).await;
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-cache"
            );
            assert!(
                to_bytes(response.into_body(), MAX_UPLOAD)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(
            post(&app, "cerro", "cerro-secret", sample(ImageFormat::Png))
                .await
                .status(),
            StatusCode::OK
        );
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        assert_eq!(
            read(State(app.clone()), Path("cerro".into()), headers)
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            remove(
                State(app.clone()),
                Path("cerro".into()),
                self::headers("cerro-secret")
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            json(list(State(app.clone())).await).await,
            serde_json::json!({})
        );
        let response = read(State(app.clone()), Path("cerro".into()), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        cleanup(app, path);
    }

    #[tokio::test]
    async fn invalid_or_oversized_upload_does_not_replace_an_existing_picture() {
        let (app, path) = fixture();
        assert_eq!(
            post(&app, "cerro", "cerro-secret", sample(ImageFormat::Png))
                .await
                .status(),
            StatusCode::OK
        );
        let original = picture(&app.store.lock().unwrap(), "cerro").unwrap();
        assert_eq!(
            post(&app, "cerro", "cerro-secret", b"not an image".to_vec())
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        // No Content-Length header is needed to enforce this bound.
        assert_eq!(
            post(&app, "cerro", "cerro-secret", vec![0; MAX_UPLOAD + 1])
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            picture(&app.store.lock().unwrap(), "cerro").unwrap(),
            original
        );
        cleanup(app, path);
    }

    #[tokio::test]
    async fn pictures_from_removed_accounts_are_not_publicly_listed_or_served() {
        let (app, path) = fixture();
        save(
            &app.store.lock().unwrap(),
            "removed",
            Some(&sample(ImageFormat::Png)),
        )
        .unwrap();
        assert_eq!(
            json(list(State(app.clone())).await).await,
            serde_json::json!({})
        );
        assert_eq!(
            read(State(app.clone()), Path("removed".into()), HeaderMap::new())
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        cleanup(app, path);
    }
}
