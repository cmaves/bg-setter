#![feature(div_duration)]
mod ffi_image;
mod shm_img;


use xcb::{Connection,ConnError,Pixmap,Screen,Window};
use xcb::randr;
//use xcb::ffi::XCB_COPY_FROM_PARENT;
//use shm_img;
use std::convert::TryInto;
use std::time::{Duration,Instant};
use std::thread::sleep;
use std::slice::from_raw_parts_mut;
use image::{Pixel,RgbImage};


pub struct XBgSetter<'b> {
	conn: &'b xcb::Connection,
	gc: xcb::Gcontext,
	shm_img: shm_img::Image<'b>,
	roots: Vec<Root>,
	verbose: bool,
	xset: Option<xcb::Atom>,
	eset: Option<xcb::Atom>
}

pub struct Root {
	index: usize,
	root: xcb::Window,
	width: u16,
	height: u16,
	pid: xcb::Pixmap,
	sizes: Vec<Display>
}
#[derive(Debug)]
struct Display {
	width: u16,
	height: u16,
	x: i16,
	y: i16
}

impl <'b> XBgSetter<'b> {
	pub fn new(conn: &'b xcb::Connection) -> Result<Self, BgError> {

		let xset = xcb::intern_atom(&conn, false, "_XROOTPMAP_ID").get_reply().ok()
			.map(|x| {x.atom()});
		let eset = xcb::intern_atom(&conn, false, "ESETROOT_PMAP_ID").get_reply().ok()
			.map(|x| {x.atom()});
		// setup XCB Graphics context
		let setup = conn.get_setup();
		let image = shm_img::create(&conn, 24, 240, 240).unwrap();
		assert!(shm_img::is_native(&conn, &image));
		let id = conn.generate_id();
		if let Some(v) = setup.roots().next() {
			xcb::create_gc(&conn, id, v.root(), &[]);
			let mut ret = XBgSetter { conn: conn, gc: id,  shm_img: image, 
				roots: Vec::new(), verbose: false, xset: xset, eset: eset };
			ret.refresh_roots();
			Ok(ret)
		} else {
			Err(BgError::NoRoot)
		}
	}
	pub fn set_verbose(&mut self, verbose: bool) { self.verbose = verbose; }
	pub fn refresh_roots(&mut self)  {
		for root in self.roots.drain(..) {
			xcb::free_pixmap(&self.conn, root.pid);
		}
		for root in self.conn.get_setup().roots().enumerate() {
			let (u, root_screen) = root;
			let root = root_screen.root();
			let pid = self.conn.generate_id();
			xcb::create_pixmap_checked(&self.conn, root_screen.root_depth(), pid, 
				root, root_screen.width_in_pixels(), 
				root_screen.height_in_pixels()).request_check().unwrap();
			let mut r = Root { root: root ,pid: pid, index: u, sizes: Vec::new(),
				width: root_screen.width_in_pixels(), 
				height: root_screen.height_in_pixels() };
			self.get_sizes(&mut r, root);
			self.roots.push(r);
			xcb::change_window_attributes(&self.conn, root, &[(xcb::CW_EVENT_MASK, xcb::EVENT_MASK_RESIZE_REDIRECT as u32)]);
		}
	}
	fn get_sizes(&self, root: &mut Root, w: xcb::Window)  {
		let sr = randr::get_screen_resources_current(&self.conn, w).get_reply().unwrap();
		let ct = sr.config_timestamp();
		let outs = sr.outputs();
		eprintln!("{:?}",outs); 
		for out in outs {
			let info = randr::get_output_info(&self.conn, *out, ct).get_reply().unwrap();
			let crtc = info.crtc();
			if info.connection() == 0x01 || crtc == xcb::NONE {
				continue;
			}
			let info = randr::get_crtc_info(&self.conn, crtc, ct)
				.get_reply().unwrap();
			root.sizes.push(Display { x: info.x(), y: info.y(), width: info.width(), 
				height: info.height() } );
		}
		eprintln!("sizes {:?}", root.sizes);
	}
	pub fn count(&self) -> usize {
		self.roots.len()
	}
	pub fn get_display_count(&self, index: usize) -> usize { 
		self.roots[index].sizes.len() 
	}
	pub fn get_displays(&self, index: usize) -> Vec<(u16, u16)> {
		self.roots[index].sizes.iter().map(|x|{(x.width, x.height)}).collect()
	}
	pub fn check_resized(&self) -> bool {
		// TODO add possible print for errors 
		if let Err(e) = self.conn.has_error() { panic!("Conn has error {:?}", e); }
		loop {
			if let Some(event) = self.conn.poll_for_event() {
				match event.response_type() {
					0 => unsafe { 
						let ge: xcb::GenericError = std::mem::transmute(event);
						eprintln!("X11 error: {}", ge.error_code());
					}
					xcb::RESIZE_REQUEST => {
						let event: &xcb::ResizeRequestEvent = unsafe { 
							// the if statement guarantees this is safe
							xcb::cast_event(&event)
						};
						for root in self.roots.iter() {
							if event.window() == root.root {
								if self.verbose { println!("Resizing"); }
								return true;
							}
						}
					},
					v => if self.verbose { eprintln!("Unknown message: {:?}", v); }
				}
			} else {
				return false;
			}
		}
	}
	pub fn check_resized_refresh(&mut self) -> bool {
		if self.check_resized() { self.refresh_roots(); true } else { false }
	}
	fn resize_shm(&mut self, rgb: &RgbImage) -> Result<(), BgError>{
		let mut smi_width = self.shm_img.width();
		let mut smi_height = self.shm_img.height();
		let rgb_width: u16 = rgb.width().try_into().map_err(|_|{BgError::ToLargeRgb})?;
		let rgb_height: u16 = rgb.height().try_into().map_err(|_|{BgError::ToLargeRgb})?;
		if smi_width < rgb_width || smi_height < rgb_height {
			if smi_width < rgb_width { smi_width = rgb_width; }
			if smi_height < rgb_height { smi_height = rgb_height; }
			self.shm_img = shm_img::create(&self.conn, 24, 
					smi_width, smi_height).unwrap();
		}
		Ok(())

	}
	fn set_window_bg(&self, win: u32, pid: u32) {
		if let Some(prop) = self.xset {
			xcb::change_property(&self.conn, xcb::PROP_MODE_REPLACE as u8, win,
			prop, xcb::ATOM_PIXMAP, 32, &[pid]);
		}
		if let Some(prop) = self.eset {
			xcb::change_property(&self.conn, xcb::PROP_MODE_REPLACE as u8, win,
			prop, xcb::ATOM_PIXMAP, 32, &[pid]);
		}
		xcb::change_window_attributes(&self.conn, win,
			&[(xcb::CW_BACK_PIXMAP, pid)]);
		//eprintln!("root window: 0x{:X}", win);
		self.conn.flush();
	}
	pub fn replace_abs(&mut self, screen: usize, x: u16, y: u16, rgb: &RgbImage) {
		eprintln!("replace_abs() x: {}, y: {}", x, y);
		assert!(screen < self.count(), 
			"Tried to get nonexistent screen! Did you need to call refresh_roots?");
		let root = &self.roots[screen];
		let pid = root.pid;
		let root_window = root.root;
		let x = x;
		let y = y;
		if x < root.width && y < root.height {
			self.resize_shm(rgb);
				// put the pixels in  the image 
			//self.put
			self.put_image_shm(rgb, None, 0);
			/*
			let order = self.shm_img.byte_order();
			for (x, y, p) in rgb.enumerate_pixels() {
				let channels = p.channels();
				let red  = channels[0];
				let green = channels[1];
				let blue = channels[2];
				let z = rgb_to_zpix(red, green, blue, order);
				self.shm_img.put(x, y, z);
			}
			*/	
			// put info
			shm_img::put(&self.conn, pid, self.gc, &self.shm_img,
				0, 0, x as i16, y as i16, rgb.width() as u16, rgb.height() as u16, false).unwrap();
			self.set_window_bg(root_window ,pid);
		} else {
			if self.verbose { eprintln!("tried to set outside"); }
		}

	}

	pub fn replace(&mut self, screen: usize, display: usize, x: u16, y: u16, rgb: &RgbImage) {
		eprintln!("display {}", display);
		let (x, y) = self.screen_to_abs(screen, display, x, y);
		self.replace_abs(screen, x, y , rgb);

	}
	pub fn fade_abs(&mut self, screen: usize, x: u16, y: u16, rgb: &RgbImage, secs: f32) 
		-> Result<(), BgError> 
	{
		assert!(screen < self.count(), 
			"Tried to get nonexistent screen! Did you need to call refresh_roots?");
		let root = &self.roots[screen];
		let pid = root.pid;
		let root_window = root.root;
		let iters = (if secs >= 4.0 { 256.0 } else { secs * 64.0 }).round();
		let tpi = Duration::from_secs_f32(secs / iters);
		eprintln!("fade_abs() x: {}, y: {}, tpi: {:?}", x, y, tpi);
		if x < root.width && y < root.height {
			let geo = xcb::get_geometry(&self.conn,pid).get_reply().unwrap();
			let rgb_width: u16 = rgb.width().try_into().map_err(|_|{BgError::ToLargeRgb})?;
			let rgb_height: u16 = rgb.height().try_into().map_err(|_|{BgError::ToLargeRgb})?;
			let mut width = geo.width() - x;
			let mut height = geo.height() - y;
			if rgb_width < width { width = rgb_width; }
			if rgb_height < height { height = rgb_height; }
			if width != self.shm_img.width() || height != self.shm_img.height() {
				self.shm_img = shm_img::create(&self.conn, 24, width, height).unwrap();
			}
				// put the pixels in  the image 
			shm_img::get(&self.conn, pid, &mut self.shm_img, x as i16, y as i16, 0xffffffff)
				.unwrap();
			let mut diffs: Vec<(f32, f32, f32)> 
				= Vec::with_capacity((width as usize) * (height as usize));
			let order = self.shm_img.byte_order();
			for j in 0..height {
				for i in 0..width {
					let z = self.shm_img.get(i as u32, j as u32);	
					let (red, green, blue) = zpix_to_rgb(z, order);
					let channels = rgb.get_pixel(i as u32, j as u32).channels();
					let red = (red as i16).wrapping_sub(channels[0] as i16) as f32 / iters;
					let green = (green as i16).wrapping_sub(channels[1] as i16) as f32 / iters;
					let blue = (blue as i16).wrapping_sub(channels[2] as i16) as f32 / iters;
					diffs.push((red, green, blue));
				}
			}
			//eprintln!("{:?}", diffs);
			let start = Instant::now();
			let iters = iters as u32;
			let mut i = iters;
			eprintln!("width {}, height {}", width, height);
			let mut count = 0;
			while i > 0 {
				self.put_image_shm(rgb, Some(&diffs), i);
				shm_img::put(&self.conn, pid, self.gc, &self.shm_img, 0, 0, 
					x as i16, y as i16, width as u16, height as u16, false);

				self.set_window_bg(root_window, pid);
				let iter_end = start + tpi * (iters - i + 1);
				let now = Instant::now();
				if let Some(sleep_dur) = iter_end.checked_duration_since(now) {
					sleep(sleep_dur);
					i -= 1;
				} else {
					i = i.saturating_sub((
						now.duration_since(iter_end).div_duration_f32(tpi) as u32) + 1);
				}
				count += 1;
			}
			let elapsed= Instant::now().duration_since(start);
			eprintln!("It took {:?} secs to run {} iters ({} fps)", elapsed, count, 
				count as f32 / elapsed.as_secs_f32());
			Ok(())
		} else {
			if self.verbose { eprintln!("tried to set outside"); }
			Ok(())
		}
	}
	pub fn fade(&mut self, screen: usize, display: usize, x: u16, y: u16, rgb: &RgbImage, secs: f32) {
		let (x, y) = self.screen_to_abs(screen, display, x, y);
		self.fade_abs(screen, x, y , rgb, secs);
	}
	fn put_image_shm(&mut self, rgb: &RgbImage, diffs: Option<&[(f32, f32, f32)]>, iter: u32)
	{
		let (stride, data) = unsafe { 
			let image = &*(self.shm_img.base.0);
			(image.stride as usize, 
				from_raw_parts_mut(image.data, 
					image.stride as usize * image.height as usize))
		};
		let width = (self.shm_img.width() as usize).min(rgb.width() as usize);
		let height = (self.shm_img.height() as usize).min(rgb.height() as usize);
		//eprint!("width {}, height {}, stride {} ", width, height, stride);
		//unsafe { eprintln!("{}", (*(self.shm_img.base.0)).depth); }
		let order = self.shm_img.byte_order();
		let start = Instant::now();
		let iter = iter as f32;
		for n in 0..height {
			let row_start = stride * n;
			let row = &mut data[row_start..row_start+width*4];
			for m in 0..width {
				let channels = rgb.get_pixel(m as u32, n as u32);
				let (r, g, b) = if let Some(diffs) = diffs {
					let (r, g, b) = diffs[width * n + m];
					let (r, g, b) = (iter * r, iter * g, iter * b);
					let (r, g, b) = (r + channels[0] as f32, g + channels[1] as f32, 
						b + channels[2] as f32);
					(r.round() as u8, g.round() as u8, b.round() as u8)
				} else {
					(channels[0], channels[1], channels[2])
				};
				//let z = rgb_to_zpix(r, g, b, order);
				let mult = 4 * m;
				if order == xcb::IMAGE_ORDER_LSB_FIRST {
					row[mult + 2] = r;
					row[mult + 1] = g; 
					row[mult] = b;
				} else {
					panic!("Unimplemented byte order!");
				}

			}
		}


	}
	fn screen_to_abs(&self, screen: usize, display: usize, x: u16, y: u16) -> (u16, u16) {
		assert!(screen < self.count(), 
			"Tried to get nonexistent screen! Did you need to call refresh_roots?");
		let root = &self.roots[screen];
		let display = &root.sizes[display];
		let x = display.x as u16 + x;
		let y = display.y as u16 + y;
		(x, y)
	}
	pub fn display_dim(&self, screen: usize, display: usize) -> (u16, u16) {
		let dim = &self.roots[screen].sizes[display];
		(dim.width, dim.height)
	}

}

fn rgb_to_zpix(r: u8, g: u8, b: u8, order: u32) -> u32 {
	if order == xcb::IMAGE_ORDER_LSB_FIRST {
		(b as u32)  | ((g as u32) << 8) | ((r as u32) << 16)
	} else {
		// TODO: Add other order
		panic!("rgb_to_zpix: Unimplemented byte order");
	}
}

fn zpix_to_rgb(z: u32, order: u32) -> (u8, u8, u8) {
	//eprintln!("z: {}", z);
	if order == xcb::IMAGE_ORDER_LSB_FIRST {
		let red = (z >> 16) as u8;
		let green = (z >> 8) as u8;
		let blue = z as u8;
		(red, green, blue)
	} else {
		// TODO: Add other order
		panic!("zpix_to_rgb: Unimplemented byte order");
	}

}

#[derive(Debug)]
pub enum BgError {
	XCBConnError(ConnError),
	ToLargeRgb,
	NoRoot
}
