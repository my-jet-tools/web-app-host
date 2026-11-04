/// Description of the content we are asked to serve
pub struct ServeParams<'s> {
    /// Root folders of the content, in the order we are looking the file at.
    /// It is a single wwwroot folder - or the device folder plus the other one,
    /// when WITH_MOBILE mode is enabled
    pub folders: &'s [&'s str],
    /// The same url is served from the different device folders - caches in the middle
    /// must not mix them up
    pub vary_by_user_agent: bool,
    /// The pages may be shown in the frames of the other sites. Otherwise only the pages
    /// of the same site may frame them - so no other site can trick the user into clicking
    /// on the page it shows in the invisible frame
    pub allow_framing: bool,
}
