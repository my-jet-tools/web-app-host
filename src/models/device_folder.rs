/// Root folder of the content when WITH_MOBILE mode is enabled
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFolder {
    Desktop,
    Mobile,
}

impl DeviceFolder {
    /// Root folder of the content for the device
    pub fn www_root(&self) -> &'static str {
        match self {
            DeviceFolder::Desktop => crate::app::APP_CTX.desktop_www_root.as_str(),
            DeviceFolder::Mobile => crate::app::APP_CTX.mobile_www_root.as_str(),
        }
    }

    /// Folder we are looking the file at, if it is not found in the device one
    pub fn fallback(&self) -> Self {
        match self {
            DeviceFolder::Desktop => DeviceFolder::Mobile,
            DeviceFolder::Mobile => DeviceFolder::Desktop,
        }
    }
}
