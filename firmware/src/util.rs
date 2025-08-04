use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::ops::Deref;
use cortex_m::interrupt::Mutex;

pub struct InterruptAccessible<T>(Mutex<RefCell<MaybeUninit<T>>>);

impl<T> InterruptAccessible<T> {
    pub const fn new() -> Self {
        Self {
            0: Mutex::new(RefCell::new(MaybeUninit::uninit()))
        }
    }
}

impl<T> Deref for InterruptAccessible<T> {
    type Target = Mutex<RefCell<MaybeUninit<T>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[inline]
pub fn with<T, F, R>(ia: &InterruptAccessible<T>, f: F) -> R
where
    F: FnOnce(&mut T) -> R,
{
    cortex_m::interrupt::free(|cs| {
        unsafe {
            f(&mut ia.borrow(cs).borrow_mut().assume_init_mut())
        }
    })
}
