//! Cross-origin resource sharing
//!
//! Rocket (as of v0.2.2) does not have middleware support. Support for it is (supposedly)
//! on the way. In the mean time, we adopt an
//! [example implementation](https://github.com/SergioBenitez/Rocket/pull/141) to nest `Responders` to acheive
//! the same effect in the short run.
use std::collections::HashSet;
use std::error;
use std::fmt;
use std::str::FromStr;

use hyper::Url;
use hyper::error::ParseError;
use rocket;
use rocket::request::{self, Request, FromRequest};
use rocket::response::{self, Response, Responder};
use rocket::http::{Method, Status};
use rocket::Outcome;

// TODO: impl Responder?
#[derive(Debug)]
pub enum Error {
    MissingOrigin,
    BadOrigin(ParseError),
    MissingRequestMethod,
    BadRequestMethod(rocket::Error),
    MissingRequestHeaders,
}

impl error::Error for Error {
    fn description(&self) -> &str {
        match *self {
            Error::MissingOrigin => "The request header `Origin` is required but is missing",
            Error::BadOrigin(_) => "The request header `Origin` contains an invalid URL",
            Error::MissingRequestMethod => {
                "The request header `Access-Control-Request-Method` \
                 is required but is missing"
            }
            Error::BadRequestMethod(_) => "The request header `Access-Control-Request-Method` has an invalid value",
            Error::MissingRequestHeaders => {
                "The request header `Access-Control-Request-Headers` \
                is required but is missing"
            }
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match *self {
            Error::BadOrigin(ref e) => Some(e as &error::Error),
            _ => Some(self as &error::Error),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::BadOrigin(ref e) => fmt::Display::fmt(e, f),
            Error::BadRequestMethod(ref e) => fmt::Debug::fmt(e, f),
            _ => write!(f, "{}", error::Error::description(self)),
        }
    }
}

/// The `Origin` request header used in CORS
#[derive(Debug)]
pub struct Origin(Url);

impl FromStr for Origin {
    type Err = ParseError;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        let url = Url::parse(url)?;
        Ok(Origin(url))
    }
}

impl<'a, 'r> FromRequest<'a, 'r> for Origin {
    type Error = Error;

    fn from_request(request: &'a Request<'r>) -> request::Outcome<Self, Error> {
        match request.headers().get_one("Origin") {
            Some(origin) => {
                match Self::from_str(origin) {
                    Ok(origin) => Outcome::Success(origin),
                    Err(e) => Outcome::Failure((Status::BadRequest, Error::BadOrigin(e))),
                }
            }
            None => Outcome::Failure((Status::BadRequest, Error::MissingOrigin)),
        }
    }
}

/// The `Access-Control-Request-Method` request header
#[derive(Debug)]
pub struct AccessControlRequestMethod(Method);

impl FromStr for AccessControlRequestMethod {
    type Err = rocket::Error;

    fn from_str(method: &str) -> Result<Self, Self::Err> {
        Ok(AccessControlRequestMethod(Method::from_str(method)?))
    }
}

impl<'a, 'r> FromRequest<'a, 'r> for AccessControlRequestMethod {
    type Error = Error;

    fn from_request(request: &'a Request<'r>) -> request::Outcome<Self, Error> {
        match request.headers().get_one("Access-Control-Request-Method") {
            Some(request_method) => {
                match Self::from_str(request_method) {
                    Ok(request_method) => Outcome::Success(request_method),
                    Err(e) => Outcome::Failure((Status::BadRequest, Error::BadRequestMethod(e))),
                }
            }
            None => Outcome::Failure((Status::BadRequest, Error::MissingRequestMethod)),
        }
    }
}

/// The `Access-Control-Request-Headers` request header
#[derive(Debug)]
pub struct AccessControlRequestHeaders(HashSet<String>);

/// Will never fail
impl FromStr for AccessControlRequestHeaders {
    type Err = ();

    fn from_str(headers: &str) -> Result<Self, Self::Err> {
        let set: HashSet<String> = headers.split(',').map(|header| header.trim().to_string()).collect();
        Ok(AccessControlRequestHeaders(set))
    }
}

impl<'a, 'r> FromRequest<'a, 'r> for AccessControlRequestHeaders {
    type Error = Error;

    fn from_request(request: &'a Request<'r>) -> request::Outcome<Self, Error> {
        match request.headers().get_one("Access-Control-Request-Headers") {
            Some(request_headers) => {
                match Self::from_str(request_headers) {
                    Ok(request_headers) => Outcome::Success(request_headers),
                    Err(()) => unreachable!("`AccessControlRequestHeaders::from_str` should never fail"),
                }
            }
            None => Outcome::Failure((Status::BadRequest, Error::MissingRequestHeaders)),
        }
    }
}


/// The CORS type, which implements `Responder`. This type allows
/// you to request resources from another domain.
pub struct CORS<R> {
    responder: R,
    allow_origin: String,
    allow_credentials: bool,
    expose_headers: HashSet<String>,
    max_age: Option<usize>,
    allow_methods: HashSet<Method>,
    allow_headers: HashSet<String>,
}

// pub type PreflightCORS = CORS<()>;

// impl PreflightCORS {
//     /// Consumes origin for which it will allow to use `CORS`
//     /// and return a basic origin `CORS`
//     pub fn preflight(origin: &'static str) -> PreflightCORS {
//         CORS::origin((), origin)
//     }
// }

impl<'r, R: Responder<'r>> CORS<R> {
    /// Consumes responder and returns CORS with any origin
    pub fn any(responder: R) -> CORS<R> {
        CORS::origin(responder, "*")
    }

    /// Consumes the responder and origin and returns basic CORS
    pub fn origin(responder: R, origin: &str) -> CORS<R> {
        CORS {
            responder: responder,
            allow_origin: origin.to_string(),
            allow_credentials: false,
            expose_headers: HashSet::new(),
            max_age: None,
            allow_methods: HashSet::new(),
            allow_headers: HashSet::new(),
        }
    }

    /// Consumes the CORS, set allow_credentials to
    /// new value and returns changed CORS
    pub fn credentials(mut self, value: bool) -> CORS<R> {
        self.allow_credentials = value;
        self
    }

    /// Consumes the CORS, set expose_headers to
    /// passed headers and returns changed CORS
    pub fn exposed_headers(mut self, headers: &[&str]) -> CORS<R> {
        self.expose_headers = headers.into_iter().map(|s| s.to_string()).collect();
        self
    }

    /// Consumes the CORS, set max_age to
    /// passed value and returns changed CORS
    pub fn max_age(mut self, value: Option<usize>) -> CORS<R> {
        self.max_age = value;
        self
    }

    /// Consumes the CORS, set allow_methods to
    /// passed methods and returns changed CORS
    pub fn methods(mut self, methods: &[Method]) -> CORS<R> {
        self.allow_methods = methods.into_iter().cloned().collect();
        self
    }

    /// Consumes the CORS, set allow_headers to
    /// passed headers and returns changed CORS
    pub fn headers(mut self, headers: &[&str]) -> CORS<R> {
        self.allow_headers = headers.into_iter().map(|s| s.to_string()).collect();
        self
    }
}

impl<'r, R: Responder<'r>> Responder<'r> for CORS<R> {
    fn respond(self) -> response::Result<'r> {
        let mut response = Response::build_from(self.responder.respond()?)
            .raw_header("Access-Control-Allow-Origin", self.allow_origin)
            .finalize();

        if self.allow_credentials {
            response.set_raw_header("Access-Control-Allow-Credentials", "true");
        } else {
            response.set_raw_header("Access-Control-Allow-Credentials", "false");
        }

        if !self.expose_headers.is_empty() {
            let headers: Vec<_> = self.expose_headers.into_iter().collect();
            let headers = headers.join(", ");

            response.set_raw_header("Access-Control-Expose-Headers", headers);
        }

        if !self.allow_methods.is_empty() {
            let methods: Vec<_> = self.allow_methods
                .into_iter()
                .map(|m| m.as_str())
                .collect();
            let methods = methods.join(", ");

            response.set_raw_header("Access-Control-Allow-Methods", methods);
        }

        if self.max_age.is_some() {
            let max_age = self.max_age.unwrap();
            response.set_raw_header("Access-Control-Max-Age", max_age.to_string());
        }

        Ok(response)
    }
}

#[cfg(test)]
#[allow(unmounted_route)]
mod tests {
    use std::collections::HashSet;
    use std::str::FromStr;

    use hyper;
    use rocket;
    use rocket::testing::MockRequest;
    use rocket::http::Method::*;
    use rocket::http::{Header, Status};

    use cors::*;

    #[get("/hello")]
    fn hello() -> CORS<&'static str> {
        CORS::any("Hello, world!")
    }

    #[get("/request_headers")]
    fn request_headers(origin: Origin,
                       method: AccessControlRequestMethod,
                       headers: AccessControlRequestHeaders)
                       -> String {
        let Origin(origin) = origin;
        let AccessControlRequestMethod(method) = method;
        let AccessControlRequestHeaders(headers) = headers;
        format!("{}\n{}\n{}", origin, method, headers.iter().cloned().collect::<Vec<String>>().join(", "))
    }

    #[test]
    fn origin_header_parsing() {
        let url = "https://foo.bar.xyz";
        not_err!(Origin::from_str(url));

        let url = "https://foo.bar.xyz/path/somewhere"; // this should never really be used
        not_err!(Origin::from_str(url));

        let url = "invalid_url";
        is_err!(Origin::from_str(url));
    }

    #[test]
    fn request_method_parsing() {
        let method = "POST";
        let parsed_method = not_err!(AccessControlRequestMethod::from_str(method));
        assert_matches!(parsed_method, AccessControlRequestMethod(Method::Post));

        let method = "options";
        let parsed_method = not_err!(AccessControlRequestMethod::from_str(method));
        assert_matches!(parsed_method, AccessControlRequestMethod(Method::Options));

        let method = "INVALID";
        is_err!(AccessControlRequestMethod::from_str(method));
    }

    #[test]
    fn request_headers_parsing() {
        let headers = ["foo", "bar", "baz"];
        let parsed_headers = not_err!(AccessControlRequestHeaders::from_str(&headers.join(", ")));
        let expected_headers: HashSet<String> = headers.iter().map(|s| s.to_string()).collect();
        let AccessControlRequestHeaders(actual_headers) = parsed_headers;
        assert_eq!(actual_headers, expected_headers);
    }

    #[test]
    fn smoke_test() {
        let rocket = rocket::ignite().mount("/", routes![hello]);
        let mut req = MockRequest::new(Get, "/hello");
        let mut response = req.dispatch_with(&rocket);

        assert_eq!(Status::Ok, response.status());
        let body_str = response.body().and_then(|body| body.into_string());
        let values: Vec<_> = response.header_values("Access-Control-Allow-Origin").collect();
        assert_eq!(values, vec!["*"]);
        assert_eq!(body_str, Some("Hello, world!".to_string()));
    }

    #[test]
    fn request_headers_round_trip_smoke_test() {
        let rocket = rocket::ignite().mount("/", routes![request_headers]);
        let origin_header = Header::from(not_err!(hyper::header::Origin::from_str("https://foo.bar.xyz")));
        let method_header = Header::from(hyper::header::AccessControlRequestMethod(hyper::method::Method::Get));
        let request_headers = hyper::header::AccessControlRequestHeaders(vec![FromStr::from_str("accept-language")
                                                                                  .unwrap(),
                                                                              FromStr::from_str("X-Ping").unwrap()]);
        let request_headers = Header::from(request_headers);
        let mut req = MockRequest::new(Get, "/request_headers")
            .header(origin_header)
            .header(method_header)
            .header(request_headers);
        let mut response = req.dispatch_with(&rocket);

        assert_eq!(Status::Ok, response.status());
        let body_str = not_none!(response.body().and_then(|body| body.into_string()));
        let expected_body = r#"https://foo.bar.xyz/
GET
accept-language, X-Ping"#;
        assert_eq!(expected_body, body_str);
    }
}
