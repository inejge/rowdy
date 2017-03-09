#![feature(plugin)]
#![plugin(rocket_codegen)]

extern crate hyper;
extern crate jwt;
#[macro_use]
extern crate log;
#[macro_use]
extern crate rocket; // we are using the "log_!" macros which are redefined from `log`'s
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate uuid;

#[cfg(test)]
#[macro_use]
mod test;
pub mod cors;

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use std::ops::Deref;

use rocket::State;
use rocket::http::Method::*;
use serde::{Serialize, Serializer, Deserialize, Deserializer};
use serde::de;

/// Wrapper around `hyper::Url` with `Serialize` and `Deserialize` implemented
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct Url(hyper::Url);

impl Deref for Url {
    type Target = hyper::Url;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromStr for Url {
    type Err = hyper::error::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Url(hyper::Url::from_str(s)?))
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}

impl Serialize for Url {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where S: Serializer
    {
        serializer.serialize_str(self.0.as_str())
    }
}

impl Deserialize for Url {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where D: Deserializer
    {
        struct UrlVisitor;
        impl de::Visitor for UrlVisitor {
            type Value = Url;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a valid URL string")
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
                where E: de::Error
            {
                Ok(Url(hyper::Url::from_str(&value).map_err(|e| E::custom(format!("{}", e)))?))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                where E: de::Error
            {
                Ok(Url(hyper::Url::from_str(value).map_err(|e| E::custom(format!("{}", e)))?))
            }
        }

        deserializer.deserialize_string(UrlVisitor)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Configuration {
    pub allowed_origins: cors::AllowedOrigins,
}

impl Configuration {
    /// Panics if any URL is invalid.
    pub fn to_cors_options(&self,
                           allowed_methods: &HashSet<rocket::http::Method>,
                           allowed_headers: &HashSet<String>)
                           -> cors::Options {

        cors::Options {
            allowed_origins: self.allowed_origins.clone(),
            allowed_methods: allowed_methods.clone(),
            allowed_headers: allowed_headers.clone(),
        }
    }
}

/// Implement a simple Deref from `From` to `To` where `From` is a newtype struct containing `To`
macro_rules! impl_deref {
    ($f:ty, $t:ty) => {
        impl Deref for $f {
            type Target = $t;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    }
}

struct HelloCorsOptions(cors::Options);
impl_deref!(HelloCorsOptions, cors::Options);

const HELLO_METHODS: &[rocket::http::Method] = &[Get];
const HELLO_HEADERS: &'static [&'static str] = &[];

#[options("/")]
fn hello_options(origin: cors::Origin,
                 method: cors::AccessControlRequestMethod,
                 options: State<HelloCorsOptions>)
                 -> Result<cors::Response<()>, cors::Error> {
    options.preflight(&origin, &method, None)
}

#[get("/")]
fn hello(origin: cors::Origin, options: State<HelloCorsOptions>) -> Result<cors::Response<&'static str>, cors::Error> {
    options.respond("Hello world", &origin)
}

pub fn launch(config: Configuration) {
    let hello_options =
        HelloCorsOptions(config.to_cors_options(&HELLO_METHODS.iter().cloned().collect(),
                                                &HELLO_HEADERS.iter().map(|s| s.to_string()).collect()));
    rocket::ignite().mount("/", routes![hello, hello_options]).manage(hello_options).launch();
}
