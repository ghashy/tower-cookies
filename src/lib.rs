//! A cookie manager middleware built on top of [tower].
//!
//! # Example
//!
//! With [axum]:
//!
//! ```rust,no_run
//! use axum::{handler::get, Router};
//! use std::net::SocketAddr;
//! use tower_cookies::{Cookie, CookieManagerLayer, Cookies};
//!
//! # #[cfg(feature = "axum")]
//! #[tokio::main]
//! async fn main() {
//!     let app = Router::new()
//!         .route("/", get(handler))
//!         .layer(CookieManagerLayer::new());
//!
//!     let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
//!     axum::Server::bind(&addr)
//!         .serve(app.into_make_service())
//!         .await
//!         .unwrap();
//! }
//! # #[cfg(not(feature = "axum"))]
//! # fn main() {}
//!
//! async fn handler(cookies: Cookies) -> &'static str {
//!     cookies.add(Cookie::new("hello_world", "hello_world"));
//!
//!     "Check your cookies."
//! }
//! ```
//!
//! A complete CRUD cookie example in [examples/counter.rs][example]
//!
//! [axum]: https://crates.io/crates/axum
//! [tower]: https://crates.io/crates/tower
//! [example]: https://github.com/imbolc/tower-cookies/blob/main/examples/counter.rs

#![warn(clippy::all, missing_docs, nonstandard_style, future_incompatible)]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

use cookie::CookieJar;
use http::HeaderValue;
use parking_lot::Mutex;
use std::sync::Arc;

#[doc(inline)]
pub use self::service::{CookieManager, CookieManagerLayer};

pub use cookie::Cookie;

#[cfg(feature = "axum")]
#[cfg_attr(docsrs, doc(cfg(feature = "axum")))]
mod extract;

pub mod service;

/// A parsed on-demand cookie jar.
#[derive(Clone, Debug)]
pub struct Cookies {
    inner: Arc<Mutex<Inner>>,
}

impl Cookies {
    fn new(header: Option<HeaderValue>) -> Self {
        let inner = Inner {
            header,
            ..Default::default()
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Adds [`Cookie`] to this jar. If a [`Cookie`] with the same name already exists, it is
    /// replaced with provided cookie.
    pub fn add(&self, cookie: Cookie<'static>) {
        let mut inner = self.inner.lock();
        inner.changed = true;
        inner.jar().add(cookie);
    }

    /// Returns the [`Cookie`] with the given name. Returns [`None`] if it doesn't exist.
    pub fn get(&self, name: &str) -> Option<Cookie> {
        let mut inner = self.inner.lock();
        inner.changed = true;
        inner.jar().get(name).cloned()
    }

    /// Removes [`Cookie`] from this jar.
    pub fn remove(&self, cookie: Cookie<'static>) {
        let mut inner = self.inner.lock();
        inner.changed = true;
        inner.jar().remove(cookie);
    }

    /// Returns all the [`Cookie`]s present in this jar.
    ///
    /// This method collects [`Cookie`]s into a vector instead of iterating through them to
    /// minimize the mutex locking time.
    pub fn list(&self) -> Vec<Cookie> {
        let mut inner = self.inner.lock();
        inner.jar().iter().cloned().collect()
    }
}

#[derive(Debug, Default)]
struct Inner {
    header: Option<HeaderValue>,
    jar: Option<CookieJar>,
    changed: bool,
}

impl Inner {
    fn jar(&mut self) -> &mut CookieJar {
        if self.jar.is_none() {
            let jar = self
                .header
                .as_ref()
                .and_then(|h| std::str::from_utf8(h.as_bytes()).ok())
                .map(|s| jar_from_str(s))
                .unwrap_or_default();
            self.jar = Some(jar);
        }
        self.jar.as_mut().unwrap()
    }
}

fn jar_from_str(s: &str) -> CookieJar {
    let mut jar = CookieJar::new();
    for cookie_str in s.split(';').map(str::trim) {
        if let Ok(cookie) = cookie::Cookie::parse_encoded(cookie_str) {
            jar.add_original(cookie.into_owned());
        }
    }
    jar
}

#[cfg(all(test, feature = "axum"))]
mod tests {
    use crate::{CookieManagerLayer, Cookies};
    use axum::{
        body::{Body, BoxBody},
        handler::get,
        routing::BoxRoute,
        Router,
    };
    use cookie::Cookie;
    use http::{header, Request};
    use tower::ServiceExt;

    fn app() -> Router<BoxRoute> {
        Router::new()
            .route(
                "/list",
                get(|cookies: Cookies| async move {
                    let mut items = cookies
                        .list()
                        .iter()
                        .map(|c| format!("{}={}", c.name(), c.value()))
                        .collect::<Vec<_>>();
                    items.sort();
                    items.join(", ")
                }),
            )
            .route(
                "/add",
                get(|cookies: Cookies| async move {
                    cookies.add(Cookie::new("baz", "3"));
                    cookies.add(Cookie::new("spam", "4"));
                }),
            )
            .route(
                "/remove",
                get(|cookies: Cookies| async move {
                    cookies.remove(Cookie::new("foo", ""));
                }),
            )
            .layer(CookieManagerLayer::new())
            .boxed()
    }

    async fn body_string(body: BoxBody) -> String {
        let bytes = hyper::body::to_bytes(body).await.unwrap();
        String::from_utf8_lossy(&bytes).into()
    }

    #[tokio::test]
    async fn read_cookies() {
        let req = Request::builder()
            .uri("/list")
            .header(header::COOKIE, "foo=1; bar=2")
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        assert_eq!(body_string(res.into_body()).await, "bar=2, foo=1");
    }

    #[tokio::test]
    async fn add_cookies() {
        let req = Request::builder()
            .uri("/add")
            .header(header::COOKIE, "foo=1; bar=2")
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let mut hdrs: Vec<_> = res.headers().get_all(header::SET_COOKIE).iter().collect();
        hdrs.sort();
        assert_eq!(hdrs, ["baz=3", "spam=4"]);
    }

    #[tokio::test]
    async fn remove_cookies() {
        let req = Request::builder()
            .uri("/remove")
            .header(header::COOKIE, "foo=1; bar=2")
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let mut hdrs = res.headers().get_all(header::SET_COOKIE).iter();
        let hdr = hdrs.next().unwrap().to_str().unwrap();
        assert!(hdr.starts_with("foo=; Max-Age=0"));
        assert_eq!(hdrs.next(), None);
    }
}
