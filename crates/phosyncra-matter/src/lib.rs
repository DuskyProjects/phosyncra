use anyhow::{Context, Result, anyhow, bail};
use phosyncra_core::LightState;
use std::{fmt, path::PathBuf, str::FromStr, time::Duration};
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatterTarget {
    pub node_id: String,
    pub endpoint: u16,
}

impl MatterTarget {
    pub fn new(node_id: impl Into<String>, endpoint: u16) -> Result<Self> {
        let node_id = node_id.into();
        validate_node_id(&node_id)?;
        Ok(Self { node_id, endpoint })
    }
}

impl fmt::Display for MatterTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.node_id, self.endpoint)
    }
}

impl FromStr for MatterTarget {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let (node_id, endpoint) = value.rsplit_once(':').ok_or_else(|| {
            anyhow!("Matter target must be NODE_ID:ENDPOINT, for example 0x1234:1")
        })?;
        let endpoint = endpoint
            .parse::<u16>()
            .with_context(|| format!("invalid Matter endpoint {endpoint:?}"))?;

        Self::new(node_id, endpoint)
    }
}

#[derive(Debug, Clone)]
pub struct ChipTool {
    binary: PathBuf,
    commissioner_name: Option<String>,
    storage_directory: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ChipToolOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ChipToolOutput {
    pub fn combined(&self) -> String {
        match (self.stdout.trim().is_empty(), self.stderr.trim().is_empty()) {
            (false, false) => format!("{}\n{}", self.stdout.trim(), self.stderr.trim()),
            (false, true) => self.stdout.trim().to_string(),
            (true, false) => self.stderr.trim().to_string(),
            (true, true) => String::new(),
        }
    }

    pub fn is_timeout(&self) -> bool {
        self.combined().contains("CHIP Error 0x00000032: Timeout")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatterColorLevel {
    pub level: u8,
    pub hue: u8,
    pub saturation: u8,
    pub transition_ds: u16,
}

impl MatterColorLevel {
    pub fn from_light_state(state: LightState) -> Self {
        Self {
            level: scale_unit(state.brightness),
            hue: scale_hue(state.hue_degrees),
            saturation: scale_unit(state.saturation),
            transition_ds: duration_to_deciseconds(state.transition),
        }
    }
}

impl Default for ChipTool {
    fn default() -> Self {
        Self::new("chip-tool")
    }
}

impl ChipTool {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            commissioner_name: None,
            storage_directory: None,
        }
    }

    pub fn with_commissioner_name(mut self, commissioner_name: impl Into<String>) -> Self {
        self.commissioner_name = Some(commissioner_name.into());
        self
    }

    pub fn with_storage_directory(mut self, storage_directory: impl Into<PathBuf>) -> Self {
        self.storage_directory = Some(storage_directory.into());
        self
    }

    pub fn binary(&self) -> &PathBuf {
        &self.binary
    }

    pub fn storage_directory(&self) -> Option<&PathBuf> {
        self.storage_directory.as_ref()
    }

    /// Checks that chip-tool can be spawned. A non-zero exit status is accepted
    /// because invoking chip-tool with no command may print usage and fail.
    pub async fn doctor(&self) -> Result<ChipToolOutput> {
        self.run_raw(&[]).await
    }

    pub async fn discover_commissionables(&self) -> Result<ChipToolOutput> {
        let output = self.run_raw(&["discover", "commissionables"]).await?;
        if output.status == 0 || output.is_timeout() {
            return Ok(output);
        }

        checked_output(output)
    }

    pub async fn commission_code(&self, node_id: &str, setup_code: &str) -> Result<ChipToolOutput> {
        validate_node_id(node_id)?;
        if setup_code.trim().is_empty() {
            bail!("Matter setup code cannot be empty");
        }

        self.run_checked(&["pairing", "code", node_id, setup_code])
            .await
    }

    pub async fn on(&self, target: &MatterTarget) -> Result<ChipToolOutput> {
        self.run_checked(&["onoff", "on", &target.node_id, &target.endpoint.to_string()])
            .await
    }

    pub async fn off(&self, target: &MatterTarget) -> Result<ChipToolOutput> {
        self.run_checked(&[
            "onoff",
            "off",
            &target.node_id,
            &target.endpoint.to_string(),
        ])
        .await
    }

    pub async fn set_level(
        &self,
        target: &MatterTarget,
        level: u8,
        transition_ds: u16,
    ) -> Result<ChipToolOutput> {
        self.run_checked(&[
            "levelcontrol",
            "move-to-level",
            &level.to_string(),
            &transition_ds.to_string(),
            "0",
            "0",
            &target.node_id,
            &target.endpoint.to_string(),
        ])
        .await
    }

    pub async fn set_hue_saturation(
        &self,
        target: &MatterTarget,
        hue: u8,
        saturation: u8,
        transition_ds: u16,
    ) -> Result<ChipToolOutput> {
        self.run_checked(&[
            "colorcontrol",
            "move-to-hue-and-saturation",
            &hue.to_string(),
            &saturation.to_string(),
            &transition_ds.to_string(),
            "0",
            "0",
            &target.node_id,
            &target.endpoint.to_string(),
        ])
        .await
    }

    /// Diagnostic/validation path only. This launches multiple chip-tool
    /// processes and is intentionally not used for music-synchronized output.
    pub async fn apply_light_state(
        &self,
        target: &MatterTarget,
        state: LightState,
    ) -> Result<Vec<ChipToolOutput>> {
        let converted = MatterColorLevel::from_light_state(state);
        let mut outputs = Vec::with_capacity(3);

        if converted.level == 0 {
            outputs.push(self.off(target).await?);
            return Ok(outputs);
        }

        outputs.push(self.on(target).await?);
        outputs.push(
            self.set_level(target, converted.level, converted.transition_ds)
                .await?,
        );
        outputs.push(
            self.set_hue_saturation(
                target,
                converted.hue,
                converted.saturation,
                converted.transition_ds,
            )
            .await?,
        );

        Ok(outputs)
    }

    async fn run_checked(&self, args: &[&str]) -> Result<ChipToolOutput> {
        checked_output(self.run_raw(args).await?)
    }

    async fn run_raw(&self, args: &[&str]) -> Result<ChipToolOutput> {
        let mut command = Command::new(&self.binary);
        command.args(args);

        if let Some(commissioner_name) = &self.commissioner_name {
            command.arg("--commissioner-name").arg(commissioner_name);
        }

        if let Some(storage_directory) = &self.storage_directory {
            command.arg("--storage-directory").arg(storage_directory);
        }

        let output = command.output().await.with_context(|| {
            format!(
                "failed to launch {}; install/build the official Matter chip-tool or pass its path explicitly",
                self.binary.display()
            )
        })?;

        Ok(ChipToolOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn checked_output(output: ChipToolOutput) -> Result<ChipToolOutput> {
    if output.status != 0 {
        let detail = output.combined();
        if detail.is_empty() {
            bail!("chip-tool exited with status {}", output.status);
        }
        bail!("chip-tool exited with status {}: {detail}", output.status);
    }

    Ok(output)
}

fn validate_node_id(node_id: &str) -> Result<()> {
    let value = node_id.trim();
    if value.is_empty() {
        bail!("Matter node ID cannot be empty");
    }

    let valid = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit())
    } else {
        value.chars().all(|c| c.is_ascii_digit())
    };

    if !valid {
        bail!("Matter node ID must be decimal or 0x-prefixed hexadecimal");
    }

    Ok(())
}

fn scale_unit(value: f32) -> u8 {
    let clamped = value.clamp(0.0, 1.0);
    (clamped * 254.0).round() as u8
}

fn scale_hue(degrees: f32) -> u8 {
    let normalized = degrees.rem_euclid(360.0);
    (normalized / 360.0 * 254.0).round() as u8
}

fn duration_to_deciseconds(duration: Duration) -> u16 {
    let deciseconds = duration.as_millis().div_ceil(100);
    deciseconds.min(u16::MAX as u128) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decimal_and_hex_targets() {
        assert_eq!(
            "1234:1".parse::<MatterTarget>().unwrap(),
            MatterTarget {
                node_id: "1234".into(),
                endpoint: 1,
            }
        );

        assert_eq!(
            "0x1234:42".parse::<MatterTarget>().unwrap(),
            MatterTarget {
                node_id: "0x1234".into(),
                endpoint: 42,
            }
        );
    }

    #[test]
    fn rejects_invalid_target() {
        assert!("abc:1".parse::<MatterTarget>().is_err());
        assert!("1234".parse::<MatterTarget>().is_err());
    }

    #[test]
    fn recognizes_chip_tool_timeout() {
        let output = ChipToolOutput {
            status: 1,
            stdout: String::new(),
            stderr: "CHIP Error 0x00000032: Timeout".into(),
        };

        assert!(output.is_timeout());
    }

    #[test]
    fn converts_light_state_to_matter_ranges() {
        let converted = MatterColorLevel::from_light_state(LightState {
            brightness: 0.5,
            hue_degrees: 180.0,
            saturation: 1.0,
            transition: Duration::from_millis(250),
        });

        assert_eq!(converted.level, 127);
        assert_eq!(converted.hue, 127);
        assert_eq!(converted.saturation, 254);
        assert_eq!(converted.transition_ds, 3);
    }

    #[test]
    fn hue_wraps_cleanly() {
        assert_eq!(scale_hue(360.0), 0);
        assert_eq!(scale_hue(-180.0), 127);
    }
}
