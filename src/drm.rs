use crate::{layout, pixel, stride};
use core::convert::TryFrom;
use core::ops::Range;

/// A direct rendering manager format info.
///
/// The format information describes the byte layout of pixels of that particular type.
///
/// This structure is a cleaned-up version of the one present in the Linux kernel rendering
/// subsystem, with deprecated fields removed. It is not a `Layout` itself since it is not
/// guaranteed to be internally consistent, it does not yet describe width and height or it might
/// otherwise not be supported. Try to convert it to a `DynLayout`.
///
/// See: the Linux kernel header `drm/drm_fourcc.h` and
///
/// https://www.kernel.org/doc/html/latest/gpu/drm-kms.html#c.drm_format_info
#[derive(Clone, Copy, Debug, Hash)]
pub struct DrmFormatInfo {
    /// The 4CC format identifier.
    pub format: FourCC,
    /// The number of image color planes (1 to 3).
    pub num_planes: u8,
    /// Number of bytes per block (per plane).
    ///
    /// Blocks are defined as a rectangle of pixels which are stored next to each other in a byte
    /// aligned memory region. Since we have no backwards compatibility considerations there is no
    /// `cpp` member.
    ///
    /// For formats that are intended to be used only with non-linear modifiers char_per_block must
    /// be 0 in the generic format table.
    pub char_per_block: [u8; 4],
    /// The width of a block in pixels.
    pub block_w: [u8; 4],
    /// The height of a block in pixels.
    pub block_h: [u8; 4],
    /// The horizontal chroma subsampling factor.
    pub hsub: u8,
    /// The vertical chroma subsampling factor.
    pub vsub: u8,
    /// Does the format embed an alpha component?
    pub has_alpha: bool,
    /// Is it a YUV format?
    pub is_yuv: bool,
}

struct PlaneInfo {
    /// The 4CC of the whole buffer format.
    format: FourCC,
    /// Characters per block of this plane.
    char_per_block: u8,
    /// The width of a block in pixels.
    block_w: u8,
    /// The height of a block in pixels.
    block_h: u8,
    /// The horizontal chroma subsampling factor.
    hsub: u8,
    /// The vertical chroma subsampling factor.
    vsub: u8,
    /// Does the format embed an alpha component?
    has_alpha: bool,
    /// Is it a YUV format?
    is_yuv: bool,
}

/// A descriptor for a single frame buffer.
///
/// In Linux, used to request new buffers or reallocation of buffers. Here, we use it similarly as
/// the builder type to fallibly construct a `DrmFramebuffer`, a complete layout descriptor for one
/// sized image.
///
/// See: the Linux kernel header `drm/drm_mode.h`.
#[derive(Clone, Copy, Debug, Hash)]
pub struct DrmFramebufferCmd {
    pub width: u32,
    pub height: u32,
    pub fourcc: FourCC,
    pub flags: i32,
    pub pitches: [u32; 4],
    pub offsets: [u32; 4],
    pub modifier: [u64; 4],
}

/// The filled-in info about a frame buffer.
///
/// This is equivalent to `drm_framebuffer`, minus the kernel internal stuff.
pub(crate) struct DrmFramebuffer {
    pub format: DrmFormatInfo,
    pub pitches: [u32; 4],
    pub offsets: [u32; 4],
    pub modifier: u64,
    pub width: u32,
    pub height: u32,
    /// A bit mask for which modifiers are actually to be enabled. All 0 for now.
    pub flags: i32,
}

/// A direct rendering manager format info that is supported as a layout.
///
/// You can't edit this format in-place. This ensures that a bunch of pre-computation are always
/// fresh. It might be relaxed later when we find a strategy to ensure this through other means.
pub struct DrmLayout {
    /// The frame buffer layout, checked for internal consistency.
    pub(crate) info: DrmFramebuffer,
    pub(crate) element: layout::Element,
    pub(crate) total_len: usize,
}

/// The index of a plane in a frame buffer.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlaneIdx {
    First = 1,
    Second = 2,
    Third = 3,
}

/// The layout of one plane of a DRM buffer.
pub struct PlaneLayout {
    format: PlaneInfo,
    pitch: u32,
    offset: u32,
    modifier: u64,
    width: u32,
    height: u32,
}

/// An error converting an info into a supported layout.
pub struct BadDrmError {
    _private: (),
}

fn round_up_div(dimension: u32, div: u8) -> u32 {
    let div = u32::from(div);
    dimension / div + if dimension % div == 0 { 0 } else { 1 }
}

/// A 4CC format identifier.
///
/// This exist to define the common formats as constants and to typify the conversion and
/// representation of values involved. The code is always stored as little endian.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FourCC(u32);

impl DrmFormatInfo {
    /// Default values for an info of a format that has 1×1 blocks, i.e. each pixel is stored
    /// individually. Some info is left out to require it being filled in.
    const PIXEL1_TEMPLATE: Self = DrmFormatInfo {
        format: FourCC::INVALID,
        num_planes: 0,
        char_per_block: [0; 4],
        block_w: [1; 4],
        block_h: [1; 4],
        hsub: 1,
        vsub: 1,
        has_alpha: false,
        is_yuv: false,
    };

    /// Create a layout with particular dimensions.
    ///
    /// This is a partial function to represent that not all descriptors can be convert to a
    /// possible dynamic layouts. No successful conversion will get removed across SemVer
    /// compatible versions.
    pub fn into_layout(self, width: u32, height: u32) -> Option<layout::DynLayout> {
        None
    }

    fn plane_width(self, width: u32, idx: PlaneIdx) -> Option<u32> {
        // If this one of the subsampled yuv planes.
        let width = if self.is_yuv && idx != PlaneIdx::First {
            round_up_div(width, self.vsub)
        } else {
            width
        };
        let idx = idx.to_index();
        let width = round_up_div(width, self.block_w[idx]);
        Some(width)
    }

    fn plane_height(self, height: u32, idx: PlaneIdx) -> Option<u32> {
        // If this one of the subsampled yuv planes.
        let height = if self.is_yuv && idx != PlaneIdx::First {
            round_up_div(height, self.hsub)
        } else {
            height
        };
        let idx = idx.to_index();
        let height = round_up_div(height, self.block_h[idx]);
        Some(height)
    }
}

impl PlaneIdx {
    const PLANES: [PlaneIdx; 3] = [PlaneIdx::First, PlaneIdx::Second, PlaneIdx::Third];
    pub fn to_index(self) -> usize {
        self as usize - 1
    }
}

impl DrmLayout {
    /// Try to construct a layout from a filled request.
    ///
    /// Due to limited support we enforce a number of extra conditions:
    /// * Modifier must be `0`, for all planes.
    /// * Only YUV can be sub sampled.
    pub fn new(info: &DrmFramebufferCmd) -> Result<Self, BadDrmError> {
        const DEFAULT_ERR: BadDrmError = BadDrmError { _private: () };
        let format_info = info.fourcc.info()?;
        usize::try_from(info.width).map_err(|_| DEFAULT_ERR)?;
        usize::try_from(info.height).map_err(|_| DEFAULT_ERR)?;

        if format_info.num_planes < 1 || format_info.num_planes > 3 {
            return Err(DEFAULT_ERR);
        }

        let element = info.fourcc.block_element().ok_or(DEFAULT_ERR)?;

        let modifier = info.modifier[0];
        if info.modifier.iter().any(|&m| m != modifier) {
            // All modifiers must be the same (and as later enforced 0 since we don't support
            // vendor specific codes at the moment).
            return Err(DEFAULT_ERR);
        }

        let mut last_plane_end = 0;
        let planes = PlaneIdx::PLANES[..usize::from(format_info.num_planes)]
            .iter()
            .enumerate();

        for (idx, &plane) in planes {
            if info.modifier[idx] != 0 {
                return Err(DEFAULT_ERR);
            }

            if format_info.char_per_block[idx] == 0 {
                return Err(DEFAULT_ERR);
            }

            if format_info.block_w[idx] == 0 {
                return Err(DEFAULT_ERR);
            }

            if format_info.block_h[idx] == 0 {
                return Err(DEFAULT_ERR);
            }

            if info.offsets[idx] < last_plane_end {
                // Only planes in order supported.
                return Err(DEFAULT_ERR);
            }

            let width = format_info
                .plane_width(info.width, plane)
                .ok_or(DEFAULT_ERR)?;
            let height = format_info
                .plane_height(info.height, plane)
                .ok_or(DEFAULT_ERR)?;

            let char_per_line = u32::from(format_info.char_per_block[idx])
                .checked_mul(width)
                .ok_or(DEFAULT_ERR)?;

            if info.pitches[idx] < char_per_line {
                return Err(DEFAULT_ERR);
            }

            let char_for_plane = info.pitches[idx].checked_mul(height).ok_or(DEFAULT_ERR)?;

            last_plane_end = info.offsets[idx]
                .checked_add(char_for_plane)
                .ok_or(DEFAULT_ERR)?;
        }

        // Validates that all indices are valid as planes are ordered.
        let total_len = usize::try_from(last_plane_end).map_err(|_| DEFAULT_ERR)?;

        if !format_info.is_yuv && (format_info.hsub != 1 || format_info.vsub != 1) {
            // subsampling only supported for yuv.
            return Err(DEFAULT_ERR);
        }

        if format_info.hsub > 4 || !format_info.hsub.is_power_of_two() {
            return Err(DEFAULT_ERR);
        }

        if format_info.vsub > 4 || !format_info.vsub.is_power_of_two() {
            return Err(DEFAULT_ERR);
        }

        let descriptor = DrmFramebuffer {
            format: format_info,
            pitches: info.pitches,
            offsets: info.offsets,
            modifier,
            width: info.width,
            height: info.height,
            flags: 0,
        };

        Ok(DrmLayout {
            info: descriptor,
            element,
            total_len,
        })
    }

    /// Get the FourCC of this layout.
    pub fn fourcc(&self) -> FourCC {
        self.info.format.format
    }

    /// Get the layout of the nth plane of this frame buffer.
    pub fn plane(&self, plane_idx: PlaneIdx) -> Option<PlaneLayout> {
        let idx = plane_idx.to_index();

        if self.info.format.char_per_block[idx] == 0
            || self.info.format.block_w[idx] == 0
            || self.info.format.block_h[idx] == 0
        {
            // Not a Plane in the sense we're looking for.
            // TODO: this is not supported (we don't accept it in the constructor) and we might
            // want to make that distinction clear. Good for now though for forward compatible.
            return None;
        }

        Some(PlaneLayout {
            format: PlaneInfo {
                format: self.info.format.format,
                char_per_block: self.info.format.char_per_block[idx],
                block_w: self.info.format.block_w[idx],
                block_h: self.info.format.block_h[idx],
                hsub: self.info.format.hsub,
                vsub: self.info.format.vsub,
                has_alpha: self.info.format.has_alpha,
                is_yuv: self.info.format.is_yuv,
            },
            pitch: self.info.pitches[idx],
            offset: self.info.offsets[idx],
            modifier: self.info.modifier,
            width: self
                .info
                .format
                .plane_width(self.info.width, plane_idx)
                .unwrap(),
            height: self
                .info
                .format
                .plane_height(self.info.height, plane_idx)
                .unwrap(),
        })
    }

    /// The apparent width as a usize, as validated in constructor.
    fn width(&self) -> usize {
        self.info.width as usize
    }

    /// The apparent height as a usize, as validated in constructor.
    fn height(&self) -> usize {
        self.info.height as usize
    }
}

impl PlaneLayout {
    /// Get the FourCC of this layout.
    pub fn fourcc(&self) -> FourCC {
        self.format.format
    }

    fn byte_range(&self) -> Range<usize> {
        let start = self.offset as usize;
        let len = self.height() * self.pitch as usize;
        start..start + len
    }

    fn element(&self) -> layout::Element {
        todo!()
    }

    fn width(&self) -> usize {
        self.width as usize
    }

    fn height(&self) -> usize {
        self.height as usize
    }
}

impl FourCC {
    /* Relevant formats according to Linux header `uapi/drm/drm_fourcc.h` */
    /// The constant denoting an invalid format, e.g. signalling a missing format.
    pub const INVALID: Self = FourCC(0);
    /// Single 8 bpp grey color.
    pub const C8: Self = FourCC::from(*b"C8  ");

    /* 8 bpp rgb */
    /// 8bpp rgb with 3 bits red, 3 bits green, 2 bits blue.
    pub const RGB332: Self = FourCC::from(*b"RGB8");
    /// 8bpp rgb with 2 bits red, 3 bits green, 3 bits blue.
    pub const BGR332: Self = FourCC::from(*b"BGR8");

    /* 16 bpp rgb */
    /// 16 bpp xrgb with 4 bits each.
    pub const XRGB444: Self = FourCC::from(*b"XR12");
    /// 16 bpp xbgr with 4 bits each.
    pub const XBGR444: Self = FourCC::from(*b"XB12");
    /// 16 bpp rgbx with 4 bits each.
    pub const RGBX444: Self = FourCC::from(*b"RX12");
    /// 16 bpp bgrx with 4 bits each.
    pub const BGRX444: Self = FourCC::from(*b"BX12");

    const fn from(arr: [u8; 4]) -> Self {
        // FourCC(u32::from_be_bytes(arr)); not yet stable as const-fn
        FourCC(arr[0] as u32 | (arr[1] as u32) << 8 | (arr[2] as u32) << 16 | (arr[3] as u32) << 24)
    }

    pub fn info(self) -> Result<DrmFormatInfo, BadDrmError> {
        let mut info = match self {
            FourCC::C8 => DrmFormatInfo {
                num_planes: 1,
                char_per_block: [1, 0, 0, 0],
                ..DrmFormatInfo::PIXEL1_TEMPLATE
            },
            FourCC::RGB332 | FourCC::BGR332 => DrmFormatInfo {
                num_planes: 1,
                char_per_block: [1, 0, 0, 0],
                ..DrmFormatInfo::PIXEL1_TEMPLATE
            },
            FourCC::XRGB444 | FourCC::XBGR444 | FourCC::RGBX444 | FourCC::BGRX444 => {
                DrmFormatInfo {
                    num_planes: 1,
                    char_per_block: [2, 0, 0, 0],
                    ..DrmFormatInfo::PIXEL1_TEMPLATE
                }
            }
            _ => return Err(BadDrmError { _private: () }),
        };
        info.format = self;
        Ok(info)
    }

    /// The element describing each block (atomic unit) of the described layout.
    pub fn block_element(self) -> Option<layout::Element> {
        Some(match self {
            FourCC::C8 | FourCC::RGB332 | FourCC::BGR332 => pixel::constants::U8.into(),
            FourCC::XRGB444 | FourCC::XBGR444 | FourCC::RGBX444 | FourCC::BGRX444 => {
                pixel::constants::U16.into()
            }
            // No element that fits.
            _ => return None,
        })
    }
}

impl layout::Layout for DrmLayout {
    fn byte_len(&self) -> usize {
        self.total_len
    }
}

impl layout::Layout for PlaneLayout {
    fn byte_len(&self) -> usize {
        self.byte_range().end
    }
}

impl stride::Strided for PlaneLayout {
    fn strided(&self) -> stride::StrideLayout {
        let element = self.element();
        let width = self.width();
        let height = self.height();
        let matrix = layout::Matrix::from_width_height(element, width, height)
            .expect("Fits in memory because the plane does");
        stride::StrideLayout::with_row_major(matrix)
    }
}
