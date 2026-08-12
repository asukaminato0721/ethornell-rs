/// Exact 32-bit target layout for one installed input region (`0xc4` bytes).
///
/// Unknown fields preserve offset names. Semantic aliases such as bitmap or
/// selected bitmap are included only where target accesses have established
/// them.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputRegionDescriptorLayout32 {
    pub unknown_00: i32,
    pub unknown_04: i32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub unknown_18: i32,
    pub unknown_1c: i32,
    pub bitmap_id: i32,
    pub unknown_24: i32,
    /// Bitmap used when this region is the group's current/hover item.
    /// Target sub_4499F0 selects this field (offset 0x28) when a6 != 0 and
    /// the region index equals the group's current region.  This is distinct
    /// from the additional pressed/action bitmap at offset 0x2c.
    pub selected_bitmap_id: i32,
    /// Additional bitmap used by the target when action/pressed state and
    /// current-item state are both active (sub_44BA40, offset 0x2c).
    pub unknown_2c: i32,
    pub format_bitmap_id: i32,
    pub unknown_34_to_a4: [i32; 29],
    pub action_fields_a8_to_bc: [i32; 6],
    pub flags: u32,
}

impl Default for InputRegionDescriptorLayout32 {
    fn default() -> Self {
        Self {
            unknown_00: 0,
            unknown_04: 0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            unknown_18: 0,
            unknown_1c: 0,
            bitmap_id: 0,
            unknown_24: 0,
            selected_bitmap_id: 0,
            unknown_2c: 0,
            format_bitmap_id: 0,
            unknown_34_to_a4: [0; 29],
            action_fields_a8_to_bc: [0; 6],
            flags: 0,
        }
    }
}

/// Exact 32-bit target input-group layout (`0x40` bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputGroupDescriptorLayout32 {
    pub region_count: u16,
    pub group_flags: u16,
    pub unknown_04: u32,
    pub regions: u32,
    pub selected_region: i32,
    pub reserved_10: i32,
    pub unknown_14_to_3c: [i32; 11],
}

/// Exact 32-bit target root input configuration (`0x28` bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputConfigLayout32 {
    pub group_count: i32,
    pub groups: u32,
    pub initial_group: i32,
    pub unknown_0c: i32,
    pub unknown_10: i32,
    pub unknown_14: i32,
    pub mode_bits: i32,
    pub unknown_1c: i32,
    pub unknown_20: i32,
    pub unknown_24: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn target_input_layout_sizes_and_offsets_are_stable() {
        assert_eq!(size_of::<InputRegionDescriptorLayout32>(), 0xc4);
        assert_eq!(size_of::<InputGroupDescriptorLayout32>(), 0x40);
        assert_eq!(size_of::<InputConfigLayout32>(), 0x28);
        assert_eq!(offset_of!(InputRegionDescriptorLayout32, bitmap_id), 0x20);
        assert_eq!(
            offset_of!(InputRegionDescriptorLayout32, selected_bitmap_id),
            0x28
        );
        assert_eq!(
            offset_of!(InputRegionDescriptorLayout32, format_bitmap_id),
            0x30
        );
        assert_eq!(offset_of!(InputRegionDescriptorLayout32, flags), 0xc0);
        assert_eq!(offset_of!(InputGroupDescriptorLayout32, regions), 0x08);
        assert_eq!(offset_of!(InputConfigLayout32, mode_bits), 0x18);
    }
}
