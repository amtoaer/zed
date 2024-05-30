use anyhow::{anyhow, Context, Result};
use futures::{io::BufReader, stream::BoxStream, AsyncBufReadExt, AsyncReadExt, StreamExt};
use http::{AsyncBody, HttpClient, Method, Request as HttpRequest};
use isahc::config::Configurable;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::Duration;
use std::{convert::TryFrom, future::Future};

#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OpenAiCompatibleModel {
    default_model: String,
    max_token_count: usize,
}

impl OpenAiCompatibleModel {
    pub fn from_id(id: &str) -> Result<Self> {
        let parts = id
            .rsplitn(2, '|')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid model id: {}", id));
        }
        Ok(Self {
            default_model: parts[1].to_string(),
            max_token_count: parts[0].parse().context("Invalid max token count")?,
        })
    }

    pub fn id(&self) -> String {
        format!("{}|{}", self.default_model, self.max_token_count)
    }

    pub fn display_name(&self) -> &str {
        &self.default_model
    }

    pub fn max_token_count(&self) -> usize {
        self.max_token_count
    }
}
