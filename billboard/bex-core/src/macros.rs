#![allow(dead_code)]

/// Internal macro to implement ergonomic wrappers for standard BEX host utilities.
/// This abstracts the exact generated WIT paths so developers just use `http::get(url).send()`.
#[macro_export]
macro_rules! implement_bex_utils {
    ($utils_mod:path) => {
        pub mod http {
            use anyhow::{bail, Result};
            use $utils_mod as host_utils;
            use host_utils::{HttpMethod, HttpResponse, RequestOptions};

            pub struct Client {
                url: String,
                method: HttpMethod,
                headers: Vec<(String, String)>,
                body: Option<Vec<u8>>,
                timeout_seconds: Option<u32>,
            }

            impl Client {
                pub fn new(method: HttpMethod, url: &str) -> Self {
                    Self {
                        url: url.to_string(),
                        method,
                        headers: vec![("User-Agent".to_string(), "BEX Plugin".to_string())],
                        body: None,
                        timeout_seconds: Some(15),
                    }
                }

                pub fn header(mut self, key: &str, value: &str) -> Self {
                    self.headers.push((key.to_string(), value.to_string()));
                    self
                }

                pub fn body(mut self, body: Vec<u8>) -> Self {
                    self.body = Some(body);
                    self
                }

                pub fn timeout(mut self, seconds: u32) -> Self {
                    self.timeout_seconds = Some(seconds);
                    self
                }

                pub fn send(&self) -> Result<HttpResponse> {
                    let opts = RequestOptions {
                        method: self.method,
                        headers: Some(self.headers.clone()),
                        body: self.body.clone(),
                        timeout_seconds: self.timeout_seconds,
                    };

                    let res = host_utils::http_request(&self.url, &opts)
                        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;

                    Ok(res)
                }

                pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
                    let res = self.send()?;
                    if res.status < 200 || res.status >= 300 {
                        bail!("HTTP Error: {}", res.status);
                    }
                    let json_str = String::from_utf8(res.body)?;
                    Ok(serde_json::from_str(&json_str)?)
                }
            }

            pub fn get(url: &str) -> Client {
                Client::new(HttpMethod::Get, url)
            }

            pub fn post(url: &str) -> Client {
                Client::new(HttpMethod::Post, url)
            }
        }

        pub mod storage {
            use $utils_mod as host_utils;

            pub fn get(key: &str) -> Option<String> {
                host_utils::storage_get(key)
            }

            pub fn set(key: &str, value: &str) -> bool {
                host_utils::storage_set(key, value)
            }

            pub fn delete(key: &str) -> bool {
                host_utils::storage_delete(key)
            }
        }

        pub mod time {
            use $utils_mod as host_utils;

            pub fn now() -> u64 {
                host_utils::current_unix_timestamp()
            }
        }
    };
}

#[macro_export]
macro_rules! export_importer {
    ($component:ident) => {
        $crate::importer::export!($component with_types_in $crate::importer);
    };
}

#[macro_export]
macro_rules! export_lyrics {
    ($component:ident) => {
        $crate::lyrics::export!($component with_types_in $crate::lyrics);
    };
}

#[macro_export]
macro_rules! export_resolver {
    ($component:ident) => {
        $crate::resolver::export!($component with_types_in $crate::resolver);
    };
}

#[macro_export]
macro_rules! export_chart {
    ($component:ident) => {
        $crate::chart::export!($component with_types_in $crate::chart);
    };
}

#[macro_export]
macro_rules! export_scrobbler {
    ($component:ident) => {
        $crate::scrobbler::export!($component with_types_in $crate::scrobbler);
    };
}

#[macro_export]
macro_rules! export_suggestion {
    ($component:ident) => {
        $crate::suggestion::export!($component with_types_in $crate::suggestion);
    };
}

/// Implement a no-op `DiscoveryGuest` for content-resolver plugins that don't
/// provide a home-feed. Saves you from writing boilerplate empty methods.
///
/// ```rust
/// bex_core::no_discovery!(Component);
/// ```
#[macro_export]
macro_rules! no_discovery {
    ($component:ident) => {
        impl $crate::resolver::component::content_resolver::discovery::Guest for $component {
            fn get_home_sections()
                -> Result<Vec<$crate::resolver::discovery::Section>, String>
            {
                Ok(vec![])
            }
            fn load_more(
                _section_id: String,
                _page_token:  String,
            ) -> Result<Vec<$crate::resolver::types::MediaItem>, String>
            {
                Ok(vec![])
            }
        }
    };
}
