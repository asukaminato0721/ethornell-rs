use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub(crate) struct GraphGroupState {
    pub(crate) draw_enabled: bool,
    pub(crate) x: i32,
    pub(crate) y: i32,
    /// CDspObjGroup transparency parameter written by Graph90:E5 vtable+0x48.
    pub(crate) alpha_parameter: i32,
    pub(crate) members: BTreeMap<i32, (i32, i32)>,
}
