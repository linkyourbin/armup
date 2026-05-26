use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvScope {
    User,
    None,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolKind {
    ArmNoneEabiGcc,
    Clangd,
    Cmake,
    Ninja,
    XpackOpenocd,
}

impl ToolKind {
    pub fn all() -> Vec<Self> {
        vec![
            Self::ArmNoneEabiGcc,
            Self::Clangd,
            Self::Cmake,
            Self::Ninja,
            Self::XpackOpenocd,
        ]
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::ArmNoneEabiGcc => "arm-none-eabi-gcc",
            Self::Clangd => "clangd",
            Self::Cmake => "cmake",
            Self::Ninja => "ninja",
            Self::XpackOpenocd => "xpack-openocd",
        }
    }

    pub fn executable_names(self) -> &'static [&'static str] {
        match self {
            Self::ArmNoneEabiGcc => &["arm-none-eabi-gcc.exe"],
            Self::Clangd => &["clangd.exe"],
            Self::Cmake => &["cmake.exe"],
            Self::Ninja => &["ninja.exe"],
            Self::XpackOpenocd => &["openocd.exe"],
        }
    }

    pub fn matches_github_asset(self, asset_name: &str) -> bool {
        match self {
            Self::Clangd => {
                asset_name.starts_with("clangd-windows-")
                    && asset_name.ends_with(".zip")
                    && !asset_name.contains("indexing-tools")
            }
            Self::Cmake => {
                asset_name.starts_with("cmake-") && asset_name.ends_with("-windows-x86_64.zip")
            }
            Self::Ninja => asset_name.eq_ignore_ascii_case("ninja-win.zip"),
            Self::XpackOpenocd => {
                asset_name.starts_with("xpack-openocd-") && asset_name.ends_with("-win32-x64.zip")
            }
            Self::ArmNoneEabiGcc => false,
        }
    }

    pub fn picker_label(self) -> String {
        match self {
            Self::ArmNoneEabiGcc => {
                "arm-none-eabi-gcc | Arm GNU Toolchain for Cortex-M".to_string()
            }
            Self::Clangd => "clangd | C/C++ language server".to_string(),
            Self::Cmake => "cmake | Build system generator".to_string(),
            Self::Ninja => "ninja | Fast build executor".to_string(),
            Self::XpackOpenocd => "xpack-openocd | Debug probe and flash server".to_string(),
        }
    }
}

impl Display for ToolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for ToolKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "arm-none-eabi-gcc" | "gcc" | "arm-gcc" => Ok(Self::ArmNoneEabiGcc),
            "clangd" => Ok(Self::Clangd),
            "cmake" => Ok(Self::Cmake),
            "ninja" => Ok(Self::Ninja),
            "xpack-openocd" | "openocd" => Ok(Self::XpackOpenocd),
            _ => Err(format!("unsupported tool {value:?}")),
        }
    }
}
