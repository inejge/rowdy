//! LDAP Authentication module
use std::collections::HashMap;

use ldap3::{LdapConn, Scope, SearchEntry};
use strfmt::{FmtError, strfmt};
use serde_json::value;

use {Error, JsonValue, JsonMap};
use super::{Basic, AuthenticationResult};

const LDAP_SUCCESS: u8 = 0;

/// Error mapping for `FmtError`
impl From<FmtError> for Error {
    fn from(e: FmtError) -> Error {
        Error::GenericError(e.to_string())
    }
}

/// A "User" returned from LDAP. This is the same as `ldap3::SearchEntry`, but with additional traits implemented
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub struct User {
    dn: String,
    attributes: HashMap<String, Vec<String>>,
}

impl From<SearchEntry> for User {
    fn from(entry: SearchEntry) -> Self {
        Self {
            dn: entry.dn,
            attributes: entry.attrs,
        }
    }
}

/// LDAP based authenticator
///
/// Use LDAP server as the identity provider.
///
/// # Example
/// ```
/// use rowdy::auth::LdapAuthenticator;
/// let authenticator = LdapAuthenticator {
///     address: "ldap://ldap.forumsys.com".to_string(),
///     bind_dn: "cn=read-only-admin,dc=example,dc=com".to_string(),
///     bind_password: "password".to_string(),
///     search_base: "dc=example,dc=com".to_string(),
///     search_filter: Some("(uid={account})".to_string()),
///     include_attributes: vec!["cn".to_string()],
///     attributes_namespace: Some("user".to_string()),
/// };
/// ```
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub struct LdapAuthenticator {
    /// Location of the LDAP server
    pub address: String,
    /// The user that we will bind to LDAP to search for users
    pub bind_dn: String,
    /// The password that we will use to bind to LDAP to search for users
    pub bind_password: String,
    /// Base to use when searching for user. `{account}` is expanded to the user's account.
    /// Search filters _MUST_ be escaped according to RFC 4515.
    pub search_base: String,
    /// Filter to use when searching for user. `{account}` is expanded to the user's account.
    /// Search filters _MUST_ be escaped according to RFC 4515.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_filter: Option<String>,
    /// List of attributes from the LDAP Search Result Entry to be included in the JWT. The values
    /// will be placed under the `attributes_namespace` key in the JWT. Missing keys are silently ignored.
    ///
    /// When a search with LDAP is performed, a
    /// [`SearchEntry`](https://docs.rs/ldap3/0.4.4/ldap3/struct.SearchEntry.html) is returned.
    /// You can use this to configure the list of attributes that should be included in the JSON Web Token
    /// that will be provided to your applications.
    #[serde(default)]
    pub include_attributes: Vec<String>,
    /// Namespace or Key in the returned JSON Web Token to return `include_attributes` attributes in.
    /// See the documentation for the `include_attributes` for more information.
    ///
    /// If set to `None`, the keys from the LDAP attributes will be merged with that of the JWT,
    /// and any duplicate keys will result in errors as described
    /// [here](https://github.com/lawliet89/biscuit/issues/78).
    #[serde(default)]
    pub attributes_namespace: Option<String>,
}

impl LdapAuthenticator {
    /// Connects to the LDAP server
    fn connect(&self) -> Result<LdapConn, Error> {
        let connection = LdapConn::new(&self.address)?;
        Ok(connection)
    }

    /// Bind the "searcher" user
    fn searcher_bind(&self, connection: &LdapConn) -> Result<(), Error> {
        self.bind(connection, &self.bind_dn, &self.bind_password)
    }

    /// Bind the connection to some dn
    fn bind(&self, connection: &LdapConn, dn: &str, password: &str) -> Result<(), Error> {
        let (result, _) = connection.simple_bind(dn, password)?;
        if result.rc == LDAP_SUCCESS {
            Ok(())
        } else {
            Err(Error::GenericError(format!("Binding failed with reason code: {}", result.rc)))
        }
    }

    /// Search for the specified account in the directory
    fn search(&self, connection: &LdapConn, account: &str) -> Result<Vec<SearchEntry>, Error> {
        let account = Self::escape_filter(account);
        let account: HashMap<String, String> = [("account".to_string(), account)].iter().cloned().collect();
        let search_base = strfmt(&self.search_base, &account)?;
        let search_filter = match self.search_filter {
            None => "".to_string(),
            Some(ref search_filter) => strfmt(search_filter, &account)?,
        };

        // This specifies what to get back from the LDAP server
        let search_attrs_vec = vec!["cn", "dn"];
        let (results, status, _) = connection
            .search(&search_base,
                    Scope::Subtree,
                    &search_filter,
                    search_attrs_vec)?;

        if status.rc != LDAP_SUCCESS {
            Err(Error::GenericError(format!("Search failed with reason code: {}", status.rc)))?;
        }

        Ok(results.into_iter().map(SearchEntry::construct).collect())
    }

    /// Serialize a user as payload for a refresh token
    fn serialize_refresh_token_payload(user: &User) -> Result<JsonValue, Error> {
        let user = value::to_value(user)
            .map_err(|_| super::Error::AuthenticationFailure)?;
        let mut map = JsonMap::with_capacity(1);
        map.insert("user".to_string(), user);
        Ok(JsonValue::Object(map))
    }

    /// Deserialize a user from a refresh token payload
    fn deserialize_refresh_token_payload(refresh_payload: JsonValue) -> Result<User, Error> {
        match refresh_payload {
            JsonValue::Object(ref map) => {
                let user = map.get("user")
                    .ok_or_else(|| Error::Auth(super::Error::AuthenticationFailure))?;
                Ok(value::from_value(user.clone())
                       .map_err(|_| super::Error::AuthenticationFailure)?)
            }
            _ => Err(Error::Auth(super::Error::AuthenticationFailure)),
        }
    }

    /// Build an `AuthenticationResult` for a `User`
    fn build_authentication_result<T: AsRef<str>>(user: User,
                                                  include_attributes: &[T],
                                                  // TODO: namespace
                                                  include_refresh_payload: bool)
                                                  -> Result<AuthenticationResult, Error> {
        let user_dn = user.dn.clone();

        // Include LDAP attributes
        let (map, errors): (Vec<_>, Vec<_>) = include_attributes
            .iter()
            .filter(|key| user.attributes.contains_key(key.as_ref()))
            .map(|key| {
                     // Safe to unwrap
                     let attribute = &user.attributes[key.as_ref()];
                     Ok((key.as_ref().to_string(), value::to_value(attribute).map_err(|e| e.to_string())?))
                 })
            .partition(Result::is_ok);

        if !errors.is_empty() {
            let errors: Vec<String> = errors.into_iter().map(|r| r.unwrap_err()).collect();
            Err(errors.join("; "))?;
        }

        let map: JsonMap<_, _> = map.into_iter()
            .map(|tuple| {
                     // Safe to unwrap
                     tuple.unwrap()
                 })
            .collect();

        let refresh_payload = if include_refresh_payload {
            Some(Self::serialize_refresh_token_payload(&user)?)
        } else {
            None
        };

        Ok(AuthenticationResult {
               subject: user_dn,
               private_claims: JsonValue::Object(map),
               refresh_payload,
           })
    }

    /// Authenticate the user with the username/password
    pub fn verify(&self,
                  username: &str,
                  password: &str,
                  include_refresh_payload: bool)
                  -> Result<AuthenticationResult, Error> {
        let user = {
            // First, we search for the user
            let connection = self.connect()?;
            self.searcher_bind(&connection)?;
            let mut user = self.search(&connection, username)
                .map_err(|_e| super::Error::AuthenticationFailure)?;
            if user.len() != 1 {
                Err(super::Error::AuthenticationFailure)?;
            }

            user.pop().unwrap() // safe to unwrap
        };

        let user_dn = user.dn.clone();

        {
            // Attempt a bind with the user's DN and password
            let connection = self.connect()?;
            self.bind(&connection, &user_dn, password)
                .map_err(|_e| super::Error::AuthenticationFailure)?;
        }

        Self::build_authentication_result(From::from(user),
                                          self.include_attributes.as_slice(),
                                          include_refresh_payload)
    }

    /// Escape search filters according to RFC 4515. Since all strings in Rust are valid unicode,
    /// invalid unicode escaping is not done
    pub fn escape_filter(filter: &str) -> String {
        filter
            .replace("\\", "\\5c")
            .replace("*", "\\2a")
            .replace("(", "\\28")
            .replace(")", "\\29")
            .replace("\x00", "\\00")
    }
}

impl super::Authenticator<Basic> for LdapAuthenticator {
    fn authenticate(&self,
                    authorization: &super::Authorization<Basic>,
                    include_refresh_payload: bool)
                    -> Result<AuthenticationResult, Error> {
        let username = authorization.username();
        let password = authorization.password().unwrap_or_else(|| "".to_string());
        self.verify(&username, &password, include_refresh_payload)
    }

    // TODO: Implement retrieving updated information from LDAP server
    fn authenticate_refresh_token(&self, refresh_payload: &JsonValue) -> Result<AuthenticationResult, ::Error> {
        let user = Self::deserialize_refresh_token_payload(refresh_payload.clone())?;
        Self::build_authentication_result(user, self.include_attributes.as_slice(), false)
    }
}

impl super::AuthenticatorConfiguration<Basic> for LdapAuthenticator {
    type Authenticator = LdapAuthenticator;

    fn make_authenticator(&self) -> Result<Self::Authenticator, ::Error> {
        {
            // Test connection to LDAP server
            let connection = self.connect()?;
            // Test binding for user searcher
            self.searcher_bind(&connection)?;
        }

        Ok(self.clone())
    }
}

#[cfg(test)]
mod tests {
    //! These tests might intermittently fail due to Test server being inaccessible
    use auth::Authenticator;
    use super::*;

    /// Test LDAP server: http://www.forumsys.com/tutorials/integration-how-to/ldap/online-ldap-test-server/
    fn make_authenticator() -> LdapAuthenticator {
        LdapAuthenticator {
            address: "ldap://ldap.forumsys.com".to_string(),
            bind_dn: "cn=read-only-admin,dc=example,dc=com".to_string(),
            bind_password: "password".to_string(),
            search_base: "dc=example,dc=com".to_string(),
            search_filter: Some("(uid={account})".to_string()),
            include_attributes: vec!["cn".to_string()],
            attributes_namespace: Some("user".to_string()),
        }
    }

    fn make_user() -> User {
        User {
            dn: "CN=John Doe,CN=Users,DC=acme,DC=example,DC=com".to_string(),
            attributes: vec![("cn".to_string(), vec!["John Doe".to_string()]),
                             ("groups".to_string(), vec!["admins".to_string(), "user".to_string()])]
                    .into_iter()
                    .collect(),
        }
    }

    #[test]
    fn authentication() {
        let mut expected_map = JsonMap::new();
        expected_map.insert("cn".to_string(), From::from(vec!["Leonhard Euler"]));
        let expected_private_claim = JsonValue::Object(expected_map);

        let authenticator = make_authenticator();

        let result = not_err!(authenticator.verify("euler", "password", false));
        assert!(result.refresh_payload.is_none());
        assert_eq!(result.private_claims, expected_private_claim);

        let result = not_err!(authenticator.verify("euler", "password", true));
        assert!(result.refresh_payload.is_some());
        assert_eq!(result.private_claims, expected_private_claim);

        let refresh_result = not_err!(authenticator
                                          .authenticate_refresh_token(result.refresh_payload.as_ref().unwrap()));
        assert!(refresh_result.refresh_payload.is_none());

        assert_eq!(result.subject, refresh_result.subject);
    }

    #[test]
    fn attributes_are_included_correctly() {
        let result = not_err!(LdapAuthenticator::build_authentication_result(make_user(),
                                                                             vec!["cn", "groups"].as_slice(),
                                                                             false));
        let expected_attributes: JsonMap<_, _> = vec![("cn".to_string(), vec!["John Doe".to_string()]),
                                                      ("groups".to_string(),
                                                       vec!["admins".to_string(), "user".to_string()])]
                .into_iter()
                .map(|(key, value)| (key, value::to_value(value).unwrap()))
                .collect();

        assert_eq!(JsonValue::Object(expected_attributes), result.private_claims);
    }

    #[test]
    fn attributes_are_namespaced_correctly() {}

    #[test]
    fn missing_attributes_are_ignored() {
        let result = not_err!(LdapAuthenticator::build_authentication_result(make_user(),
                                                                             vec!["cn", "not_exist"].as_slice(),
                                                                             false));
        let expected_attributes: JsonMap<_, _> = vec![("cn".to_string(), vec!["John Doe".to_string()])]
                .into_iter()
                .map(|(key, value)| (key, value::to_value(value).unwrap()))
                .collect();

        assert_eq!(JsonValue::Object(expected_attributes), result.private_claims);
    }

    #[test]
    #[should_panic(expected = "AuthenticationFailure")]
    fn authentication_invalid_user() {
        let authenticator = make_authenticator();
        authenticator
            .verify("donald_trump", "password", false)
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "AuthenticationFailure")]
    fn authentication_invalid_password() {
        let authenticator = make_authenticator();
        authenticator.verify("einstein", "FTL", false).unwrap();
    }
}
