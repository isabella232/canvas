//! A module for different pixel layouts.
use crate::pixel::MaxAligned;
use crate::{AsPixel, Pixel};
use ::alloc::boxed::Box;
use core::{alloc, cmp};

/// A byte layout that only describes the user bytes.
///
/// This is a minimal implementation of the basic `Layout` trait. It does not provide any
/// additional semantics for the buffer bytes described by it. All other layouts may be converted
/// into this layout.
pub struct Bytes(pub usize);

/// Describes the byte layout of an element, untyped.
///
/// This is not so different from `Pixel` and `Layout` but is a combination of both. It has the
/// same invariants on alignment as the former which being untyped like the latter. The alignment
/// of an element must be at most that of [`MaxAligned`] and the size must be a multiple of its
/// alignment.
///
/// This type is a lower semi lattice. That is, given two elements the type formed by taking the
/// minimum of size and alignment individually will always form another valid element. This
/// operation is implemented in the [`infimum`] method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, Hash)]
pub struct Element {
    size: usize,
    align: usize,
}

/// A descriptor of the layout of image bytes.
///
/// There is no color space and no strict type interpretation here, just some mapping to required
/// bytes for such a fixed buffer and a width and height of the described image. This means that
/// the byte usage for a particular buffer needs to be independent of the content, in particular
/// can not be based on compressibility.
///
/// There is one more thing that differentiates an image from an encoded format. It is expected
/// that the image can be unfolded into some matrix of independent pixels (with potentially
/// multiple channels) without any arithmetic or conversion function. Independent here means that,
/// when supplied with the missing color space and type information, there should exist an
/// `Fn(U) -> T` that can map these pixels independently into some linear color space.
///
/// This property holds for any packed, strided or planar RGB/YCbCr/HSV format as well as chroma
/// subsampled YUV images and even raw Bayer filtered images.
pub trait Layout {
    fn byte_len(&self) -> usize;
}

/// Convert one layout to a less strict one.
///
/// In contrast to `From`/`Into` which is mostly assumed to model a lossless conversion the
/// conversion here may generalize but need not be lossless. For example, the `Bytes` layout is the
/// least descriptive layout that exists and any layout can decay into it. However, it should be
/// clear that this conversion is never lossless.
///
/// In general, a layout `L` should implement `Decay<T>` if any image with layouts of type `T` is
/// also valid for some layout of type `L`. A common example would be if a crate strictly adds more
/// information to a predefined layout, then it should also decay to that layout.
///
/// Also note that this trait is not reflexive, in contrast to `From` and `Into` which are. This
/// avoids collisions in impls. In particular, it allows adding blanket impls of the form
///
/// ```ignore
/// struct Local;
///
/// impl Trait for Local { /* … */ }
///
/// impl<T: Trait> Decay<T> for Local { /* … */ }
/// ```
///
/// Otherwise, the instantiation `T = U` would collide with the reflexive impl.
///
/// ## Design
///
/// We consider re-rebalanced coherence rules ([RFC2451]) in this design especially to define the
/// receiver type and the type parameter. Note that adding a blanket impl is a breaking change
/// except under a special rule allowed in that RFC. To quote it here:
///
/// > RFC #1023 is amended to state that adding a new impl to an existing trait is considered a
/// > breaking change unless, given impl<P1..=Pn> Trait<T1..=Tn> for T0:
/// > * At least one of the types T0..=Tn must be a local type, added in this revision. Let Ti be
/// >   the first such type.
/// > * No uncovered type parameters P1..=Pn appear in T0..Ti (excluding Ti)
/// >
/// > [...]
/// >
/// > However, the following impls would not be considered a breaking change: [...]
/// > * `impl<T> OldTrait<T> for NewType`
///
/// Let's say we want to introduce a new desciptor trait for matrix-like layouts. Then we can ship
/// a new type representing the canonical form of this matrix trait and in the same revision define
/// a blanket impl that allows other layouts to decay to it. This wouldn't be possible if the
/// parameters were swapped. We can then let this specific type (it may contain covered type
/// parameters) decay to any other previously defined layout to provide interoperability with older
/// code.
///
/// [RFC2451]: https://rust-lang.github.io/rfcs/2451-re-rebalancing-coherence.html
///
pub trait Decay<T>: Layout {
    fn decay(from: T) -> Self;
}

impl<T: Layout> Decay<T> for Bytes {
    fn decay(from: T) -> Bytes {
        Bytes(from.byte_len())
    }
}

/// Convert a layout to a stricter one.
///
/// ## Design
///
/// A comment on the design space available for this trait.
///
/// (TODO: wrong) We require that the trait is
/// implemented for the type that is _returned_. If we required that the trait be implemented for
/// the receiver then this would restrict third-parties from using it to its full potential. In
/// particular, since `Mend` is a foreign trait the coherence rules make it impossible to specify:
///
/// TODO Terminology: https://rust-lang.github.io/rfcs/2451-re-rebalancing-coherence.html
///
/// ```ignore
/// impl<T> Mend<LocalType> for T {}
/// ```
///
/// TODO: rewrite this...
///
/// ```ignore
/// impl<T> Mend<T> for LocalType {}
/// ```
///
/// The forms of evolution that we want to keep open:
/// * Introduce a new form of mending between existing layouts. For example, a new color space
///   transformation should be able to translate between existing types. Note that we will assume
///   that in such a case the type parameters do not appear uncovered in the target or the source
///   so that having either as the trait receiver (T0) allows this.
/// * An *upgrader* type should be able to mend a <T: LocalOrForeignTrait> into a chosen layout.
/// * TODO: When add a new layout type which mender types and targets do we want?
///
/// The exact form thus simply depends on expected use and the allow evolution for this crate.
/// Consider in particular this coherence/SemVer rule:
///
/// > Adding any impl with an uncovered type parameter is considered a major breaking change.
///
/// TODO
///
/// TODO: comment and consider `&self`.
///
pub trait Mend<From> {
    type Into: Layout;
    fn mend(self, from: &From) -> Self::Into;
}

/// Try to convert a layout to a stricter one.
pub trait TryMend<From> {
    type Into: Layout;
    type Err;
    fn try_mend(self, from: &From) -> Result<Self::Into, Self::Err>;
}

/// A layout that can be emptied.
///
/// This trait contains all layout types from which we can steal their memory buffer. This is
/// incredibly useful for fallible operations that change the _type_ of a buffers layout. Instead
/// of being required to take the buffer by value and return the original in case of an error they
/// can use the much natural signature:
///
/// * `fn mutate(&mut self) -> Result<Converted, Err>`
///
/// where semantics are that the buffer is unchanged in case of error but has been moved to the
/// type `Converted` in case of success. This is very similar to the method `Vec::take` and others.
///
/// It is expected that the `byte_len` is `0` after the operation.
///
/// This trait is _not_ simply a clone of `Default`. While we expect that the described image
/// contains no bytes after the operation other data such as channel count, color space
/// information, image plane order, alpha interpretation should be retained.
pub trait Take: Layout {
    fn take(&mut self) -> Self;
}

/// Describes an image coordinate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coord(pub u32, pub u32);

impl Coord {
    pub fn x(self) -> u32 {
        self.0
    }

    pub fn y(self) -> u32 {
        self.1
    }

    pub fn yx(self) -> (u32, u32) {
        (self.1, self.0)
    }

    pub fn xy(self) -> (u32, u32) {
        (self.0, self.1)
    }
}

/// A layout that is a slice of samples.
///
/// These layouts are represented with a slice of a _single_ type of samples. In particular these
/// can be addressed and mutated independently.
pub trait SampleSlice: Layout {
    /// The sample type itself.
    type Sample;

    /// Get the sample description.
    fn sample(&self) -> Pixel<Self::Sample>;

    /// The number of samples.
    ///
    /// A slice with the returned length should have the byte length returned in `byte_len`.
    fn len(&self) -> usize {
        self.byte_len() / self.sample().size()
    }
}

/// A dynamic descriptor of an image's layout.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DynLayout {
    pub(crate) repr: LayoutRepr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum LayoutRepr {
    Matrix(Matrix),
    Yuv420p(Yuv420p),
}

/// A matrix of packed pixels (channel groups).
///
/// This is a simple layout of exactly width·height homogeneous pixels. Note that it does not
/// prescribe any particular order of arrangement of these channels. Indeed, they could be in
/// column major format, in row major format, ordered according to some space filling curve, etc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Matrix {
    element: Element,
    first_dim: usize,
    second_dim: usize,
}

/// Planar chroma 2×2 block-wise sub-sampled image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Yuv420p {
    channel: Element,
    width: u32,
    height: u32,
}

/// A typed matrix of packed pixels (channel groups).
///
/// This is a strongly-typed equivalent to [`Matrix`]. See it for details.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct TMatrix<P> {
    pixel: Pixel<P>,
    first_dim: usize,
    second_dim: usize,
}

/// An error indicating that mending failed due to mismatching pixel attributes.
///
/// This struct is used when a layout with dynamic pixel information should be mended into another
/// layout with static information or a more restrictive combination of layouts. One example is the
/// conversion of a dynamic matrix into a statically typed layout.
#[derive(Debug, Default, PartialEq, Eq, Hash)]
pub struct MismatchedPixelError {
    _private: (),
}

impl Bytes {
    /// Forget all layout semantics except the number of bytes used.
    pub fn from_layout(layout: impl Layout) -> Self {
        Bytes(layout.byte_len())
    }
}

impl Element {
    /// Construct an element from a self-evident pixel.
    pub fn from_pixel<P: AsPixel>() -> Self {
        let pix = P::pixel();
        Element {
            size: pix.size(),
            align: pix.align(),
        }
    }

    /// An element with maximum size and no alignment requirements.
    ///
    /// This constructor is mainly useful for the purpose of using it as a modifier. When used with
    /// [`infimum`] it will only shrink the alignment and keep the size unchanged.
    pub const MAX_SIZE: Self = {
        Element {
            size: isize::MAX as usize,
            align: 1,
        }
    };

    /// Create an element for a fictional type with specific layout.
    ///
    /// It's up to the caller to define or use an actual type with that same layout later. This
    /// skips the check that such a type must not contain any padding and only performs the layout
    /// related checks.
    pub fn with_layout(layout: alloc::Layout) -> Option<Self> {
        if layout.align() > MaxAligned::pixel().align() {
            return None;
        }

        if layout.size() % layout.align() != 0 {
            return None;
        }

        Some(Element {
            size: layout.size(),
            align: layout.align(),
        })
    }

    /// Convert this into a type layout.
    ///
    /// This can never fail as `Element` refines the standard library layout type.
    pub fn layout(self) -> alloc::Layout {
        alloc::Layout::from_size_align(self.size, self.align).expect("Valid layout")
    }

    /// Reduce the alignment of the element.
    ///
    /// This will perform the same modification as `repr(packed)` on the element's type.
    ///
    /// # Panics
    ///
    /// This method panics if `align` is not a valid alignment.
    #[must_use = "This does not modify `self`."]
    pub fn packed(self, align: usize) -> Element {
        assert!(align.is_power_of_two());
        let align = self.align.min(align);
        Element { align, ..self }
    }

    /// Create an element having the smaller of both sizes and alignments.
    #[must_use = "This does not modify `self`."]
    pub fn infimum(self, other: Self) -> Element {
        // We still have size divisible by align. Whatever the smaller of both, it's divisible by
        // its align and thus also by the min of both alignments.
        Element {
            size: self.size.min(other.size),
            align: self.align.min(other.align),
        }
    }

    /// Get the size of the element.
    pub const fn size(self) -> usize {
        self.size
    }

    /// Get the minimum required alignment of the element.
    pub const fn align(self) -> usize {
        self.size
    }
}

impl DynLayout {
    pub fn byte_len(&self) -> usize {
        match self.repr {
            LayoutRepr::Matrix(matrix) => matrix.byte_len(),
            LayoutRepr::Yuv420p(matrix) => matrix.byte_len(),
        }
    }
}

impl Matrix {
    pub fn empty(element: Element) -> Self {
        Matrix {
            element,
            first_dim: 0,
            second_dim: 0,
        }
    }

    pub fn from_width_height(
        element: Element,
        first_dim: usize,
        second_dim: usize,
    ) -> Option<Self> {
        let max_index = first_dim.checked_mul(second_dim)?;
        let _ = max_index.checked_mul(element.size)?;

        Some(Matrix {
            element,
            first_dim,
            second_dim,
        })
    }

    /// Get the element type of this matrix.
    pub const fn element(&self) -> Element {
        self.element
    }

    /// Get the width of this matrix.
    pub const fn width(&self) -> usize {
        self.first_dim
    }

    /// Get the height of this matrix.
    pub const fn height(&self) -> usize {
        self.second_dim
    }

    /// Get the required bytes for this layout.
    pub const fn byte_len(self) -> usize {
        // Exactly this does not overflow due to construction.
        self.element.size * self.len()
    }

    /// The number of pixels in this layout
    pub const fn len(self) -> usize {
        self.first_dim * self.second_dim
    }

    /* FIXME: These methods would rely on a particular column/row major layout. Move them somewhere
     * else or make another matrix type.
        pub fn offset(self, coord1: usize, coord2: usize) -> Option<usize> {
            if self.first_dim >= coord1 || self.second_dim >= coord2 {
                None
            } else {
                Some(self.offset_unchecked(coord1, coord2))
            }
        }

        pub const fn offset_unchecked(self, coord1: usize, coord2: usize) -> usize {
            coord1 + coord2 * self.first_dim
        }

        pub fn byte_offset(self, coord1: usize, coord2: usize) -> Option<usize> {
            if self.first_dim >= coord1 || self.second_dim >= coord2 {
                None
            } else {
                Some(self.byte_offset_unchecked(coord1, coord2))
            }
        }

        pub const fn byte_offset_unchecked(self, coord1: usize, coord2: usize) -> usize {
            (coord1 + coord2 * self.first_dim) * self.element.size
        }
    */
}

impl<P> TMatrix<P> {
    pub fn empty(pixel: Pixel<P>) -> Self {
        TMatrix {
            pixel,
            first_dim: 0,
            second_dim: 0,
        }
    }

    pub fn with_matrix(pixel: Pixel<P>, matrix: Matrix) -> Option<Self> {
        if pixel.size() == matrix.element.size {
            Some(TMatrix {
                pixel,
                first_dim: matrix.first_dim,
                second_dim: matrix.second_dim,
            })
        } else {
            None
        }
    }

    pub fn into_matrix(self) -> Matrix {
        Matrix {
            element: self.pixel.into(),
            first_dim: self.first_dim,
            second_dim: self.second_dim,
        }
    }
}

impl Yuv420p {
    pub fn from_width_height(channel: Element, width: u32, height: u32) -> Option<Self> {
        use core::convert::TryFrom;
        if width % 2 != 0 || height % 2 != 0 {
            return None;
        }

        let mwidth = usize::try_from(width).ok()?;
        let mheight = usize::try_from(height).ok()?;

        let y_count = mwidth.checked_mul(mheight)?;
        let uv_count = y_count / 2;

        let count = y_count.checked_add(uv_count)?;
        let _ = count.checked_mul(channel.size)?;

        Some(Yuv420p {
            channel,
            width,
            height,
        })
    }

    pub const fn byte_len(self) -> usize {
        let ylen = (self.width as usize) * (self.height as usize) * self.channel.size;
        ylen + ylen / 2
    }
}

impl Layout for Bytes {
    fn byte_len(&self) -> usize {
        self.0
    }
}

impl Take for Bytes {
    fn take(&mut self) -> Self {
        Bytes(core::mem::take(&mut self.0))
    }
}

impl Layout for DynLayout {
    fn byte_len(&self) -> usize {
        DynLayout::byte_len(self)
    }
}

impl Layout for Matrix {
    fn byte_len(&self) -> usize {
        Matrix::byte_len(*self)
    }
}

impl Take for Matrix {
    fn take(&mut self) -> Self {
        core::mem::replace(self, Matrix::empty(self.element))
    }
}

impl<P> Layout for TMatrix<P> {
    fn byte_len(&self) -> usize {
        self.into_matrix().byte_len()
    }
}

impl<P> SampleSlice for TMatrix<P> {
    type Sample = P;
    fn sample(&self) -> Pixel<P> {
        self.pixel
    }
}

impl<P> Take for TMatrix<P> {
    fn take(&mut self) -> Self {
        core::mem::replace(self, TMatrix::empty(self.pixel))
    }
}

/// Remove the strong typing for dynamic channel type information.
impl<P> Decay<TMatrix<P>> for Matrix {
    fn decay(from: TMatrix<P>) -> Matrix {
        from.into_matrix()
    }
}

/// Try to use the matrix with a specific pixel type.
impl<P> TryMend<Matrix> for Pixel<P> {
    type Into = TMatrix<P>;
    type Err = MismatchedPixelError;

    fn try_mend(self, matrix: &Matrix) -> Result<TMatrix<P>, Self::Err> {
        TMatrix::with_matrix(self, *matrix).ok_or_else(MismatchedPixelError::default)
    }
}

/// Convert a pixel to an element, discarding the exact type information.
impl<P> From<Pixel<P>> for Element {
    fn from(pix: Pixel<P>) -> Self {
        Element {
            size: pix.size(),
            align: pix.align(),
        }
    }
}

impl<L: Layout + ?Sized> Layout for Box<L> {
    fn byte_len(&self) -> usize {
        (**self).byte_len()
    }
}

impl<L: Layout> Decay<L> for Box<L> {
    fn decay(from: L) -> Box<L> {
        Box::new(from)
    }
}

/// The partial order of elements is defined by comparing size and alignment.
///
/// This turns it into a semi-lattice structure, with infimum implementing the meet operation. For
/// example, the following comparison all hold:
///
/// ```
/// # use canvas::pixels::{U8, U16};
/// # use canvas::layout::Element;
/// let u8 = Element::from(U8);
/// let u8x2 = Element::from(U8.array2());
/// let u8x3 = Element::from(U8.array3());
/// let u16 = Element::from(U16);
///
/// assert!(u8 < u16, "due to size and alignment");
/// assert!(u8x2 < u16, "due to its alignment");
/// assert!(!(u8x3 < u16) && !(u16 < u8x3), "not comparable");
///
/// let meet = u8x3.infimum(u16);
/// assert!(meet <= u8x3);
/// assert!(meet <= u16);
/// assert!(meet == u16.packed(1), "We know it precisely here {:?}", meet);
/// ```
impl cmp::PartialOrd for Element {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        if self.size == other.size && self.align == other.align {
            Some(cmp::Ordering::Equal)
        } else if self.size <= other.size && self.align <= other.align {
            Some(cmp::Ordering::Less)
        } else if self.size >= other.size && self.align >= other.align {
            Some(cmp::Ordering::Greater)
        } else {
            None
        }
    }
}

macro_rules! bytes_from_layout {
    ($layout:path) => {
        impl From<$layout> for Bytes {
            fn from(layout: $layout) -> Self {
                Bytes::from_layout(layout)
            }
        }
    };
    (<$($bound:ident),*> $layout:ident) => {
        impl<$($bound),*> From<$layout <$($bound),*>> for Bytes {
            fn from(layout: $layout <$($bound),*>) -> Self {
                Bytes::from_layout(layout)
            }
        }
    };
}

bytes_from_layout!(DynLayout);
bytes_from_layout!(Matrix);
bytes_from_layout!(<P> TMatrix);

impl From<Matrix> for DynLayout {
    fn from(matrix: Matrix) -> Self {
        DynLayout {
            repr: LayoutRepr::Matrix(matrix),
        }
    }
}

impl From<Yuv420p> for DynLayout {
    fn from(matrix: Yuv420p) -> Self {
        DynLayout {
            repr: LayoutRepr::Yuv420p(matrix),
        }
    }
}

impl<P> From<TMatrix<P>> for Matrix {
    fn from(mat: TMatrix<P>) -> Self {
        Matrix {
            element: mat.pixel.into(),
            first_dim: mat.first_dim,
            second_dim: mat.second_dim,
        }
    }
}

impl<P> Clone for TMatrix<P> {
    fn clone(&self) -> Self {
        TMatrix { ..*self }
    }
}

impl<P> Copy for TMatrix<P> {}
