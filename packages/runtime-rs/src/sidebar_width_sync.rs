pub const MIN_SIDEBAR_WIDTH: u16 = 20;

pub fn clamp_sidebar_width(width: u16) -> u16 {
    width.max(MIN_SIDEBAR_WIDTH)
}
