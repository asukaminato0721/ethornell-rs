pub(crate) const TITLE_PENDING_CALLBACK_ADDR: u32 = 1856;
const MOUSE_CLICK_EVENT: i32 = 0x1000_0006;

pub(crate) fn clears_title_pending_callback(event: i32, payload: i32) -> bool {
    let logical_button = payload & 0xffff;
    event == MOUSE_CLICK_EVENT && (0..=8).contains(&logical_button)
}
