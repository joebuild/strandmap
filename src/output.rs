use std::{
    fmt::Display,
    io::{self, Write},
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Human,
    Json,
    Yaml,
}

impl Display for OutputFormat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Human => formatter.write_str("human"),
            Self::Json => formatter.write_str("json"),
            Self::Yaml => formatter.write_str("yaml"),
        }
    }
}

impl FromStr for OutputFormat {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "human" | "text" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            "yaml" | "yml" => Ok(Self::Yaml),
            _ => bail!("unknown output format {value:?}"),
        }
    }
}

pub fn structured<T: Serialize>(value: &T, format: OutputFormat) -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    match format {
        OutputFormat::Json => {
            serde_json::to_writer_pretty(&mut output, value).context("failed to encode JSON")?;
            writeln!(output).context("failed to write output")?;
        }
        OutputFormat::Yaml => {
            serde_yaml_ng::to_writer(&mut output, value).context("failed to encode YAML")?;
        }
        OutputFormat::Human => bail!("human output requires a command-specific renderer"),
    }
    Ok(())
}
