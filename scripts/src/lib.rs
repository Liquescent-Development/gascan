#![forbid(unsafe_code)]

use std::{collections::BTreeSet, error::Error, time::Duration};

use reqwest::{
    Url,
    blocking::{Client, Response},
    redirect::{Action, Attempt, Policy},
};

pub type DynError = Box<dyn Error + Send + Sync>;

#[derive(Clone)]
pub struct RedirectRules {
    allowed: BTreeSet<String>,
    allow_http: bool,
    max_redirects: usize,
}

impl RedirectRules {
    pub fn image_artifacts() -> Self {
        Self {
            allowed: [
                "github.com",
                "objects.githubusercontent.com",
                "release-assets.githubusercontent.com",
                "cdn.playwright.dev",
                "playwright.download.prss.microsoft.com",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            allow_http: false,
            max_redirects: 5,
        }
    }

    #[doc(hidden)]
    pub fn for_test_http_origins(
        origins: impl IntoIterator<Item = String>,
        max_redirects: usize,
    ) -> Self {
        Self {
            allowed: origins.into_iter().collect(),
            allow_http: true,
            max_redirects,
        }
    }

    fn approves(&self, url: &Url) -> bool {
        let scheme_allowed = url.scheme() == "https" || (self.allow_http && url.scheme() == "http");
        let Some(host) = url.host_str() else {
            return false;
        };
        let authority = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        };
        scheme_allowed && self.allowed.contains(&authority)
    }

    fn approve_redirect(&self, url: &Url, previous: usize) -> Result<(), DynError> {
        if previous >= self.max_redirects {
            return Err("artifact redirect limit exceeded".into());
        }
        if !self.approves(url) {
            return Err(format!("artifact redirect target is not approved: {url}").into());
        }
        Ok(())
    }
}

pub fn open_with_redirect_rules(url: &str, rules: RedirectRules) -> Result<Response, DynError> {
    let initial = Url::parse(url)?;
    if !rules.approves(&initial) {
        return Err(format!("artifact URL is not approved: {initial}").into());
    }
    let redirects = rules.clone();
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .redirect(Policy::custom(move |attempt| {
            validate_redirect(attempt, &redirects)
        }))
        .build()?;
    Ok(client.get(initial).send()?.error_for_status()?)
}

fn validate_redirect(attempt: Attempt<'_>, rules: &RedirectRules) -> Action {
    if let Err(error) = rules.approve_redirect(attempt.url(), attempt.previous().len()) {
        return attempt.error(error);
    }
    attempt.follow()
}

#[doc(hidden)]
pub fn walk_redirects_with(
    initial: &str,
    rules: RedirectRules,
    mut fetch: impl FnMut(&Url) -> Result<Option<Url>, DynError>,
) -> Result<(), DynError> {
    let mut current = Url::parse(initial)?;
    if !rules.approves(&current) {
        return Err(format!("artifact URL is not approved: {current}").into());
    }
    let mut redirects = 0;
    loop {
        let Some(next) = fetch(&current)? else {
            return Ok(());
        };
        rules.approve_redirect(&next, redirects)?;
        redirects += 1;
        current = next;
    }
}
