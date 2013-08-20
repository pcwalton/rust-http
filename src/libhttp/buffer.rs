/// Memory buffers for the benefit of `std::rt::io::net` which has slow read/write.

use std::rt::io::{Reader, Writer, Stream};
use std::rt::io::net::tcp::TcpStream;
//use std::cast::transmute_mut;
use std::cmp::min;
use std::ptr;

pub type BufTcpStream = BufferedStream<TcpStream>;

// 64KB chunks (moderately arbitrary)
static READ_BUF_SIZE: uint = 0x10000;
static WRITE_BUF_SIZE: uint = 0x10000;
// TODO: consider removing constants and giving a buffer size in the constructor

struct BufferedStream<T> {
    wrapped: T,
    read_buffer: [u8, ..READ_BUF_SIZE],
    // The current position in the buffer
    read_pos: uint,
    // The last valid position in the reader
    read_max: uint,
    write_buffer: [u8, ..WRITE_BUF_SIZE],
    write_len: uint,

    /// Some things being written may not like flush() being called yet (e.g. explicitly fail!())
    /// The BufferedReader may need to be flushed for good control, but let it provide for such
    /// cases by not calling the wrapped object's flush method in turn.
    call_wrapped_flush: bool,
}

impl<T: Reader + Writer /*Stream*/> BufferedStream<T> {
    pub fn new(stream: T, call_wrapped_flush: bool) -> BufferedStream<T> {
        BufferedStream {
            wrapped: stream,
            read_buffer: [0u8, ..READ_BUF_SIZE],
            read_pos: 0u,
            read_max: 0u,
            write_buffer: [0u8, ..WRITE_BUF_SIZE],
            write_len: 0u,
            call_wrapped_flush: call_wrapped_flush,
        }
    }
}

impl<T: Stream> Stream for BufferedStream<T>;

impl<T: Reader> BufferedStream<T> {
    /// Poke a single byte back so it will be read next. For this to make sense, you must have just
    /// read that byte. If `self.pos` is 0 and `self.max` is not 0 (i.e. if the buffer is just
    /// filled
    /// Very great caution must be used in calling this as it will fail if `self.pos` is 0.
    pub fn poke_byte(&mut self, byte: u8) {
        match (self.read_pos, self.read_max) {
            (0, 0) => self.read_max = 1,
            (0, _) => fail!("poke called when buffer is full"),
            (_, _) => self.read_pos -= 1,
        }
        self.read_buffer[self.read_pos] = byte;
    }

    #[inline]
    fn fill_buffer(&mut self) -> bool {
        assert_eq!(self.read_pos, self.read_max);
        match self.wrapped.read(self.read_buffer) {
            None => {
                self.read_pos = 0;
                self.read_max = 0;
                false
            },
            Some(i) => {
                self.read_pos = 0;
                self.read_max = i;
                true
            },
        }
    }

    /// Slightly faster implementation of read_byte than that which is provided by ReaderUtil
    /// (which just uses `read()`)
    #[inline]
    pub fn read_byte(&mut self) -> Option<u8> {
        if self.read_pos == self.read_max && !self.fill_buffer() {
            // Run out of buffered content, no more to come
            return None;
        }
        self.read_pos += 1;
        Some(self.read_buffer[self.read_pos - 1])
    }
}

impl<T: Reader> Reader for BufferedStream<T> {
    /// Read at most N bytes into `buf`, where N is the minimum of `buf.len()` and the buffer size.
    ///
    /// At present, this makes no attempt to fill its buffer proactively, instead waiting until you
    /// ask.
    fn read(&mut self, buf: &mut [u8]) -> Option<uint> {
        if self.read_pos == self.read_max && !self.fill_buffer() {
            // Run out of buffered content, no more to come
            return None;
        }
        let size = min(self.read_max - self.read_pos, buf.len());
        unsafe {
            do buf.as_mut_buf |p_dst, _len_dst| {
                do self.read_buffer.as_imm_buf |p_src, _len_src| {
                    // Note that copy_memory works on bytes; good, u8 is byte-sized
                    ptr::copy_memory(p_dst, ptr::offset(p_src, self.read_pos as int), size)
                }
            }
        }
        self.read_pos += size;
        Some(size)
    }

    /// Return whether the Reader has reached the end of the stream AND exhausted its buffer.
    fn eof(&mut self) -> bool {
        self.read_pos == self.read_max && self.wrapped.eof()
    }
}

#[unsafe_destructor]
impl<T: Writer> Drop for BufferedStream<T> {
    fn drop(&self) {
        // Clearly wouldn't be a good idea to finish without flushing!

        // TODO: blocked on https://github.com/mozilla/rust/issues/4252
        // Also compare usage of response.flush() in server.rs
        //unsafe { transmute_mut(self) }.flush();
    }
}

impl<T: Writer> Writer for BufferedStream<T> {
    fn write(&mut self, buf: &[u8]) {
        if buf.len() + self.write_len > self.write_buffer.len() {
            // This is the lazy approach which may involve two writes where it's really not
            // warranted. Maybe deal with that later.
            if self.write_len > 0 {
                self.wrapped.write(self.write_buffer.slice_to(self.write_len));
                self.write_len = 0;
            }
            self.wrapped.write(buf);
            self.write_len = 0;
        } else {
            // Safely copy buf onto the "end" of self.buffer
            unsafe {
                do buf.as_imm_buf |p_src, len_src| {
                    do self.write_buffer.as_mut_buf |p_dst, _len_dst| {
                        // Note that copy_memory works on bytes; good, u8 is byte-sized
                        ptr::copy_memory(ptr::mut_offset(p_dst, self.write_len as int),
                                         p_src, len_src)
                    }
                }
            }
            self.write_len += buf.len();
            if self.write_len == self.write_buffer.len() {
                self.wrapped.write(self.write_buffer);
                self.write_len = 0;
            }
        }
    }

    fn flush(&mut self) {
        if self.write_len > 0 {
            self.wrapped.write(self.write_buffer.slice_to(self.write_len));
            self.write_len = 0;
        }
        if self.call_wrapped_flush {
            self.wrapped.flush();
        }
    }
}
