//! # Zephyr Kernel Objects
//!
//! Zephyr has a concept of a 'kernel object' that is handled a bit magically.  In kernel mode
//! threads, these are just pointers to the data structures that Zephyr uses to manage that item.
//! In userspace, they are still pointers, but those data structures aren't accessible to the
//! thread.  When making syscalls, the kernel validates that the objects are both valid kernel
//! objects and that the are supposed to be accessible to this thread.
//!
//! In many Zephyr apps, the kernel objects in the app are defined as static, using special macros.
//! These macros make sure that the objects get registered so that they are accessible to userspace
//! (at least after that access is granted).
//!
//! There are also kernel objects that are synthesized as part of the build.  Most notably, there
//! are ones generated by the device tree.
//!
//! ## Safety
//!
//! Zephyr has traditionally not focused on safety.  Early versions of project goals, in fact,
//! emphasized performance and small code size as priorities over runtime checking of safety.  Over
//! the years, this focus has changed a bit, and Zephyr does contain some additional checking, some
//! of which is optional.
//!
//! Zephyr is still constrained at compile time to checks that can be performed with the limits
//! of the C language.  With Rust, we have a much greater ability to enforce many aspects of safety
//! at compile time.  However, there is some complexity to doing this at the interface between the C
//! world and Rust.
//!
//! There are two types of kernel objects we deal with.  There are kernel objects that are allocated
//! by C code (often auto-generated) that should be accessible to Rust.  These are mostly `struct
//! device` values, and will be handled in a devices module.  The other type are objects that
//! application code wishes to declare statically, and use from Rust code.  That is the
//! responsibility of this module.  (There will also be support for more dynamic management of
//! kernel objects, but this will be handled later).
//!
//! Static kernel objects in Zephyr are declared as C top-level variables (where the keyword static
//! means something different).  It is the responsibility of the calling code to initialize these
//! items, make sure they are only initialized once, and to ensure that sharing of the object is
//! handled properly.  All of these are concerns we can handle in Rust.
//!
//! To handle initialization, we pair each kernel object with a single atomic value, whose zero
//! value indicates [`KOBJ_UNINITIALIZED`].  There are a few instances of values that can be placed
//! into uninitialized memory in a C declaration that will need to be zero initialized as a Rust
//! static.  The case of thread stacks is handled as a special case, where the initialization
//! tracking is kept separate so that the stack can still be placed in initialized memory.
//!
//! This state goes through two more values as the item is initialized, one indicating the
//! initialization is happening, and another indicating it has finished.
//!
//! For each kernel object, there will be two types.  One, having a name of the form StaticThing,
//! and the other having the form Thing.  The StaticThing will be used in a static declaration.
//! There is a [`kobj_define!`] macro that matches declarations of these values and adds the
//! necessary linker declarations to place these in the correct linker sections.  This is the
//! equivalent of the set of macros in C, such as `K_SEM_DEFINE`.
//!
//! This StaticThing will have a single method [`init_once`] which accepts a single argument of a
//! type defined by the object.  For most objects, it will just be an empty tuple `()`, but it can
//! be whatever initializer is needed for that type by Zephyr.  Semaphores, for example, take the
//! initial value and the limit.  Threads take as an initializer the stack to be used.
//!
//! This `init_once` will initialize the Zephyr object and return the `Thing` item that will have
//! the methods on it to use the object.  Attributes such as `Sync`, and `Clone` will be defined
//! appropriately so as to match the semantics of the underlying Zephyr kernel object.  Generally
//! this `Thing` type will simply be a container for a direct pointer, and thus using and storing
//! these will have the same characteristics as it would from C.
//!
//! Rust has numerous strict rules about mutable references, namely that it is not safe to have more
//! than one mutable reference.  The language does allow multiple `*mut ktype` references, and their
//! safety depends on the semantics of what is pointed to.  In the case of Zephyr, some of these are
//! intentionally thread safe (for example, things like `k_sem` which have the purpose of
//! synchronizing between threads).  Others are not, and that is mirrored in Rust by whether or not
//! `Clone` and/or `Sync` are implemented.  Please see the documentation of individual entities for
//! details for that object.
//!
//! In general, methods on `Thing` will require `&mut self` if there is any state to manage.  Those
//! that are built around synchronization primitives, however, will generally use `&self`.  In
//! general, objects that implement `Clone` will use `&self` because there would be no benefit to
//! mutable self when the object could be cloned.
//!
//! [`kobj_define!`]: crate::kobj_define
//! [`init_once`]: StaticKernelObject::init_once

use core::{cell::UnsafeCell, mem};

use crate::sync::atomic::{AtomicUsize, Ordering};

// The kernel object itself must be wrapped in `UnsafeCell` in Rust.  This does several thing, but
// the primary feature that we want to declare to the Rust compiler is that this item has "interior
// mutability".  One impact will be that the default linker section will be writable, even though
// the object will not be declared as mutable.  It also affects the compiler as it will avoid things
// like aliasing and such on the data, as it will know that it is potentially mutable.  In our case,
// the mutations happen from C code, so this is less important than the data being placed in the
// proper section.  Many will have the link section overridden by the `kobj_define` macro.

/// A kernel object represented statically in Rust code.
///
/// These should not be declared directly by the user, as they generally need linker decorations to
/// be properly registered in Zephyr as kernel objects.  The object has the underlying Zephyr type
/// T, and the wrapper type W.
///
/// Kernel objects will have their `StaticThing` implemented as `StaticKernelObject<kobj>` where
/// `kobj` is the type of the underlying Zephyr object.  `Thing` will usually be a struct with a
/// single field, which is a `*mut kobj`.
///
/// TODO: Can we avoid the public fields with a const new method?
///
/// TODO: Handling const-defined alignment for these.
pub struct StaticKernelObject<T> {
    #[allow(dead_code)]
    /// The underlying zephyr kernel object.
    pub value: UnsafeCell<T>,
    /// Initialization status of this object.  Most objects will start uninitialized and be
    /// initialized manually.
    pub init: AtomicUsize,
}

/// Define the Wrapping of a kernel object.
///
/// This trait defines the association between a static kernel object and the two associated Rust
/// types: `StaticThing` and `Thing`.  In the general case: there should be:
/// ```
/// impl Wrapped for StaticKernelObject<kobj> {
///     type T = Thing,
///     type I = (),
///     fn get_wrapped(&self, args: Self::I) -> Self::T {
///         let ptr = self.value.get();
///         // Initizlie the kobj using ptr and possible the args.
///         Thing { ptr }
///     }
/// }
/// ```
pub trait Wrapped {
    /// The wrapped type.  This is what `init_once()` on the StaticKernelObject will return after
    /// initialization.
    type T;

    /// The wrapped type also requires zero or more initializers.  Which are represented by this
    /// type.
    type I;

    /// Initialize this kernel object, and return the wrapped pointer.
    fn get_wrapped(&self, args: Self::I) -> Self::T;
}

/// A state indicating an uninitialized kernel object.
///
/// This must be zero, as kernel objects will
/// be represetned as zero-initialized memory.
pub const KOBJ_UNINITIALIZED: usize = 0;

/// A state indicating a kernel object that is being initialized.
pub const KOBJ_INITING: usize = 1;

/// A state indicating a kernel object that has completed initialization.  This also means that the
/// take has been called.  And shouldn't be allowed additional times.
pub const KOBJ_INITIALIZED: usize = 2;

impl<T> StaticKernelObject<T>
where
    StaticKernelObject<T>: Wrapped,
{
    /// Construct an empty of these objects, with the zephyr data zero-filled.  This is safe in the
    /// sense that Zephyr we track the initialization, they start in the uninitialized state, and
    /// the zero value of the initialize atomic indicates that it is uninitialized.
    pub const fn new() -> StaticKernelObject<T> {
        StaticKernelObject {
            value: unsafe { mem::zeroed() },
            init: AtomicUsize::new(KOBJ_UNINITIALIZED),
        }
    }

    /// Get the instance of the kernel object.
    ///
    /// Will return a single wrapped instance of this object.  This will invoke the initialization,
    /// and return `Some<Wrapped>` for the wrapped containment type.
    ///
    /// If it is called an additional time, it will return None.
    pub fn init_once(&self, args: <Self as Wrapped>::I) -> Option<<Self as Wrapped>::T> {
        if let Err(_) = self.init.compare_exchange(
            KOBJ_UNINITIALIZED,
            KOBJ_INITING,
            Ordering::AcqRel,
            Ordering::Acquire)
        {
            return None;
        }
        let result = self.get_wrapped(args);
        self.init.store(KOBJ_INITIALIZED, Ordering::Release);
        Some(result)
    }
}

/// Declare a static kernel object.  This helps declaring static values of Zephyr objects.
///
/// This can typically be used as:
/// ```
/// kobj_define! {
///     static A_MUTEX: StaticMutex;
///     static MUTEX_ARRAY: [StaticMutex; 4];
/// }
/// ```
#[macro_export]
macro_rules! kobj_define {
    ($v:vis static $name:ident: $type:tt; $($rest:tt)*) => {
        $crate::_kobj_rule!($v, $name, $type);
        $crate::kobj_define!($($rest)*);
    };
    ($v:vis static $name:ident: $type:tt<$size:ident>; $($rest:tt)*) => {
        $crate::_kobj_rule!($v, $name, $type<$size>);
        $crate::kobj_define!($($rest)*);
    };
    ($v:vis static $name:ident: $type:tt<$size:literal>; $($rest:tt)*) => {
        $crate::_kobj_rule!($v, $name, $type<$size>);
        $crate::kobj_define!($($rest)*);
    };
    ($v:vis static $name:ident: $type:tt<{$size:expr}>; $($rest:tt)*) => {
        $crate::_kobj_rule!($v, $name, $type<{$size}>);
        $crate::kobj_define!($($rest)*);
    };
    () => {};
}

#[doc(hidden)]
#[macro_export]
macro_rules! _kobj_rule {
    // static NAME: StaticSemaphore;
    ($v:vis, $name:ident, StaticSemaphore) => {
        #[link_section = concat!("._k_sem.static.", stringify!($name), ".", file!(), line!())]
        $v static $name: $crate::sys::sync::StaticSemaphore =
            unsafe { ::core::mem::zeroed() };
    };

    // static NAMES: [StaticSemaphore; COUNT];
    ($v:vis, $name:ident, [StaticSemaphore; $size:expr]) => {
        #[link_section = concat!("._k_sem.static.", stringify!($name), ".", file!(), line!())]
        $v static $name: [$crate::sys::sync::StaticSemaphore; $size] =
            unsafe { ::core::mem::zeroed() };
    };

    // static THREAD: staticThread;
    ($v:vis, $name:ident, StaticThread) => {
        // Since the static object has an atomic that we assume is initialized, we cannot use the
        // default linker section Zephyr uses for Thread, as that is uninitialized.  This will put
        // it in .bss, where it is zero initialized.
        $v static $name: $crate::sys::thread::StaticThread =
            unsafe { ::core::mem::zeroed() };
    };

    // static THREAD: [staticThread; COUNT];
    ($v:vis, $name:ident, [StaticThread; $size:expr]) => {
        // Since the static object has an atomic that we assume is initialized, we cannot use the
        // default linker section Zephyr uses for Thread, as that is uninitialized.  This will put
        // it in .bss, where it is zero initialized.
        $v static $name: [$crate::sys::thread::StaticThread; $size] =
            unsafe { ::core::mem::zeroed() };
    };

    // Use indirection on stack initializers to handle some different cases in the Rust syntax.
        ($v:vis, $name:ident, ThreadStack<$size:literal>) => {
        $crate::_kobj_stack!($v, $name, $size);
    };
    ($v:vis, $name:ident, ThreadStack<$size:ident>) => {
        $crate::_kobj_stack!($v, $name, $size);
    };
    ($v:vis, $name:ident, ThreadStack<{$size:expr}>) => {
        $crate::_kobj_stack!($v, $name, $size);
    };

    // Array of stack object versions.
    ($v:vis, $name:ident, [ThreadStack<$size:literal>; $asize:expr]) => {
        $crate::_kobj_stack!($v, $name, $size, $asize);
    };
    ($v:vis, $name:ident, [ThreadStack<$size:ident>; $asize:expr]) => {
        $crate::_kobj_stack!($v, $name, $size, $asize);
    };
    ($v:vis, $name:ident, [ThreadStack<{$size:expr}>; $asize:expr]) => {
        $crate::_kobj_stack!($v, $name, $size, $asize);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! _kobj_stack {
    ($v:vis, $name: ident, $size:expr) => {
        $crate::paste! {
            // The actual stack itself goes into the no-init linker section.  We'll use the user_name,
            // with _REAL appended, to indicate the real stack.
            #[link_section = concat!(".noinit.", stringify!($name), ".", file!(), line!())]
            $v static [< $name _REAL >]: $crate::sys::thread::RealStaticThreadStack<{$crate::sys::thread::stack_len($size)}> =
                unsafe { ::core::mem::zeroed() };

            // The proxy object used to ensure initialization is placed in initialized memory.
            $v static $name: $crate::_export::KStaticThreadStack =
                $crate::_export::KStaticThreadStack::new_from(&[< $name _REAL >]);
        }
    };

    // This initializer needs to have the elements of the array initialized to fixed elements of the
    // `RealStaticThreadStack`.  Unfortunately, methods such as [`each_ref`] on the array are not
    // const and can't be used in a static initializer.  We could use a recursive macro definition
    // to perform the initialization, but this would require the array size to only be an integer
    // literal (constants aren't calculated until after macro expansion).  It may also be possible
    // to write a constructor for the array as a const fn, which would greatly simplify the
    // initialization here.
    ($v:vis, $name: ident, $size:expr, $asize:expr) => {
        $crate::paste! {
            // The actual stack itself goes into the no-init linker section.  We'll use the user_name,
            // with _REAL appended, to indicate the real stack.
            #[link_section = concat!(".noinit.", stringify!($name), ".", file!(), line!())]
            $v static [< $name _REAL >]:
                [$crate::sys::thread::RealStaticThreadStack<{$crate::sys::thread::stack_len($size)}>; $asize] =
                unsafe { ::core::mem::zeroed() };

            $v static $name: 
                [$crate::_export::KStaticThreadStack; $asize] =
                $crate::_export::KStaticThreadStack::new_from_array(&[< $name _REAL >]);
        }
    };
}
