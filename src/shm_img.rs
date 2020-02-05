
use std::ptr;
use std::ops::{Deref, DerefMut};

use xcb;
use xcb::ffi::shm::*;
use crate::ffi_image::*;
use libc::{shmget, shmat, shmdt, shmctl, IPC_CREAT, IPC_RMID};

pub struct BaseImage(pub *mut xcb_image_t);

impl Drop for BaseImage {
	fn drop(&mut self) {
		unsafe {
			xcb_image_destroy(self.0);
		}
	}
}

impl BaseImage {
	pub fn annotate(&self) {
		unsafe {
			xcb_image_annotate(self.0)
		}
	}
	pub fn width(&self) -> u16 {
		unsafe {
			(*self.0).width
		}
	}

	pub fn height(&self) -> u16 {
		unsafe {
			(*self.0).height
		}
	}
	pub fn put(&mut self, x: u32, y: u32, pixel: u32) {
		unsafe {
			xcb_image_put_pixel(self.0, x, y, pixel)
		}
	}

	pub fn get(&self, x: u32, y: u32) -> u32 {
		unsafe {
			xcb_image_get_pixel(self.0, x, y)
		}
	}
	pub fn byte_order(&self) -> xcb::ImageOrder {
		unsafe {
			(*self.0).byte_order
		}
	}
}

pub fn is_native(c: &xcb::Connection, image: &BaseImage) -> bool {
	unsafe {
		xcb_image_native(c.get_raw_conn(), image.0, 0) == image.0
	}
}

#[cfg(feature = "thread")]
unsafe impl Send for Image { }
#[cfg(feature = "thread")]
unsafe impl Sync for Image { }


pub struct Image<'conn> {
	conn: &'conn xcb::Connection,
	pub base: BaseImage,
	shm:  xcb_shm_segment_info_t,

	width:  u16,
	height: u16,
}

#[cfg(feature = "thread")]
unsafe impl Send for Image { }
#[cfg(feature = "thread")]
unsafe impl Sync for Image { }

pub fn create(c: &xcb::Connection, depth: u8, width: u16, height: u16) -> Result<Image, ()> {
	unsafe {
		let setup  = c.get_setup();
		let format = setup.pixmap_formats().find(|f| f.depth() == depth).ok_or(())?;
		let image  = xcb_image_create(width, height, xcb::IMAGE_FORMAT_Z_PIXMAP,
			format.scanline_pad(), format.depth(), format.bits_per_pixel(),
			setup.bitmap_format_scanline_unit(), setup.image_byte_order() as u32, setup.bitmap_format_bit_order() as u32,
			ptr::null_mut(), !0, ptr::null_mut());

		if image.is_null() {
			return Err(());
		}

		let id = match shmget(0, (*image).size as usize, IPC_CREAT | 0o666) {
			-1 => {
				xcb_image_destroy(image);
				return Err(());
			}

			id => id
		};

		let addr = match shmat(id, ptr::null(), 0) {
			addr if addr as isize == -1 => {
				xcb_image_destroy(image);
				shmctl(id, IPC_RMID, ptr::null_mut());

				return Err(());
			}

			addr => addr
		};

		let seg = c.generate_id();
		xcb::shm::attach(c, seg, id as u32, false);
		(*image).data = addr as *mut _;

		Ok(Image {
			conn: c,
			base: BaseImage(image),
			shm: xcb_shm_segment_info_t {
				shmseg:  seg,
				shmid:   id as u32,
				shmaddr: addr as *mut _,
			},

			width:  width,
			height: height,
		})
	}
}

impl Image<'_> {
	pub fn resize(&mut self, width: u16, height: u16) {
		assert!(width <= self.width && height <= self.height);

		unsafe {
			(*self.base.0).width  = width;
			(*self.base.0).height = height;
		}

		self.annotate();
	}

	pub fn restore(&mut self) {
		let width  = self.width;
		let height = self.height;

		self.resize(width, height);
	}

	pub fn actual_width(&self) -> u16 {
		self.width
	}

	pub fn actual_height(&self) -> u16 {
		self.height
	}
}

impl Deref for Image<'_> {
	type Target = BaseImage;

	fn deref(&self) -> &Self::Target {
		&self.base
	}
}

impl DerefMut for Image<'_> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.base
	}
}

impl Drop for Image<'_> {
	fn drop(&mut self) {
		unsafe {
			xcb_shm_detach(self.conn.get_raw_conn(), self.shm.shmseg);
			shmdt(self.shm.shmaddr as *mut _);
			shmctl(self.shm.shmid as i32, IPC_RMID, ptr::null_mut());
		}
	}
}

pub fn get<'a>(c: &xcb::Connection, drawable: xcb::Drawable, output: & mut Image<'a>, x: i16, y: i16, plane_mask: u32) -> Result<(), ()> {
	unsafe {
		if xcb_image_shm_get(c.get_raw_conn(), drawable, output.base.0, output.shm, x, y, plane_mask) != 0 {
			Ok(())
		}
		else {
			Err(())
		}
	}
}

pub fn put<'a>(c: &xcb::Connection, drawable: xcb::Drawable, gc: xcb::Gcontext, input: &'a Image<'a>, src_x: i16, src_y: i16, dest_x: i16, dest_y: i16, src_width: u16, src_height: u16, send_event: bool) -> Result<(), ()> {
	unsafe {
		if !xcb_image_shm_put(c.get_raw_conn(), drawable, gc, input.base.0, input.shm, src_x, src_y, dest_x, dest_y, src_width, src_height, send_event as u8).is_null() {
			Ok(())
		}
		else {
			Err(())
		}
	}
}

/// Fetches an area from the given drawable.
///
/// For technical reasons the `output` is resized to fit the area, to restore
/// it to its original dimensions see `Image::restore`, the shared memory is
/// untouched.
pub fn area<'a>(c: &xcb::Connection, drawable: xcb::Drawable, output: &'a mut Image<'a>, x: i16, y: i16, width: u16, height: u16, plane_mask: u32) -> Result<(), ()> {
	output.resize(width, height);
	get(c, drawable, output, x, y, plane_mask).map(|_|{})
}

