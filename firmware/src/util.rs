use core::cell::{Cell, RefCell, UnsafeCell};
use core::mem::MaybeUninit;
use core::ops::Deref;
use cortex_m::interrupt::Mutex;
use heapless::Vec;

pub struct InterruptAccessible<T>(Mutex<RefCell<MaybeUninit<T>>>);

impl<T> InterruptAccessible<T> {
    pub const fn new() -> Self {
        Self {
            0: Mutex::new(RefCell::new(MaybeUninit::uninit())),
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
    cortex_m::interrupt::free(|cs| unsafe { f(&mut ia.borrow(cs).borrow_mut().assume_init_mut()) })
}

#[derive(Clone, Copy)]
struct RingBufferPtrs {
    // Where the next byte is to be read from
    read: usize,
    // Where the next byte is to be written to
    write: usize,
}

pub struct RingBuffer<T: Default + Copy, const Len: usize> {
    data: UnsafeCell<[T; Len]>,
    read_write_ptrs: Mutex<Cell<RingBufferPtrs>>,
}

impl<T: Default + Copy, const Len: usize> RingBuffer<T, Len> {
    // Reads from buffer to target, up to index "up_to", but not including that byte,
    // starting at ptrs.read, which is included
    fn read_up_to(
        self: &Self,
        target: &mut [T],
        up_to: usize,
        ptrs: &mut RingBufferPtrs,
        num_read: &mut usize,
    ) -> bool {
        assert!(ptrs.read <= up_to);
        assert!(ptrs.read <= Len);
        assert!(up_to <= Len);

        while ptrs.read < up_to {
            if *num_read >= target.len() {
                // We ran out of space in target
                return false;
            }
            unsafe {
                // SAFETY: This is safe, because no reading may take place to the right of write buffer,
                // atleast until reaching read ptr
                target[*num_read] = (*self.data.get())[ptrs.read];
            }

            *num_read += 1;
            ptrs.read += 1
        }

        true
    }

    // Blocking writing very briefly, reads as much data as possible into destination slice
    // Returns number of elements read
    pub fn read(self: &Self, target: &mut [T]) -> usize {
        let mut ptrs = cortex_m::interrupt::free(|cs| self.read_write_ptrs.borrow(cs).get());

        let mut num_read = 0;

        if ptrs.write == ptrs.read {
            // No data to read available
            return 0;
        }

        if ptrs.write < ptrs.read {
            // We need to read to end of buffer, and then up to write ptr
            if self.read_up_to(target, Len, &mut ptrs, &mut num_read) {
                // We wrapped around
                ptrs.read = 0;
                self.read_up_to(target, ptrs.write, &mut ptrs, &mut num_read);
            }
        } else {
            // We need to read up to write ptr
            self.read_up_to(target, ptrs.write, &mut ptrs, &mut num_read);
        }

        cortex_m::interrupt::free(|cs| {
            self.read_write_ptrs.borrow(cs).set(ptrs);
        });

        num_read
    }

    // Writes data to buffer, up to index "up_to", but not including that byte,
    // starting at ptrs.write, which is included. Returns true if we reached the
    // desired end position, false otherwise
    fn write_up_to(
        self: &Self,
        data: &[T],
        up_to: usize,
        ptrs: &mut RingBufferPtrs,
        num_written: &mut usize,
    ) -> bool {
        assert!(ptrs.write <= up_to);
        assert!(ptrs.write <= Len);
        assert!(up_to <= Len);

        while ptrs.write < up_to {
            if *num_written >= data.len() {
                // We ran out of source material in data
                return false;
            }
            unsafe {
                // SAFETY: This is safe, because no reading may take place to the right of write buffer,
                // atleast until reaching read ptr
                (*self.data.get())[ptrs.write] = data[*num_written];
            }

            *num_written += 1;
            ptrs.write += 1
        }

        true
    }

    // Blocking reading very briefly, writes as much data as possible from source slice
    // Returns number of elements written
    pub fn write(self: &Self, data: &[T]) -> usize {
        let mut ptrs = cortex_m::interrupt::free(|cs| self.read_write_ptrs.borrow(cs).get());

        let mut num_written = 0;

        if ptrs.write < ptrs.read {
            // We need to write up to read ptr
            self.write_up_to(data, ptrs.read, &mut ptrs, &mut num_written);
        } else {
            // We need to write up to end of buffer, and then up to read ptr
            if self.write_up_to(data, Len, &mut ptrs, &mut num_written) {
                // We wrapped around
                ptrs.write = 0;
                self.write_up_to(data, ptrs.read, &mut ptrs, &mut num_written);
            }
        }

        cortex_m::interrupt::free(|cs| {
            self.read_write_ptrs.borrow(cs).set(ptrs);
        });

        num_written
    }

    pub fn new() -> Self {
        Self {
            data: UnsafeCell::new([T::default(); Len]),
            read_write_ptrs: Mutex::new(Cell::new(RingBufferPtrs {
                read: 0,
                write: 0,
            }))
        }
    }
}
