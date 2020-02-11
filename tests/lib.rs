use image::RgbImage;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;
use xcb;
use bg_setter::XBgSetter;

#[macro_use]extern crate lazy_static;
lazy_static! {
	static ref images: Vec<RgbImage> = {
		let paths = [
			Path::new("test_data/img1.jpg"),
			Path::new("test_data/img2.jpg"),
			Path::new("test_data/img3.jpg"),
		];
		let imgs = paths.into_iter().map(|x|{
			image::open(x).unwrap().into_rgb()
		}).collect();
		imgs
	};
}

#[test]
fn test_replace() {
	let (conn, _) = xcb::Connection::connect(None).unwrap();
	let mut bg = XBgSetter::new(&conn).unwrap();
	bg.set_verbose(true);
	let dur = Duration::from_millis(2500);
	for i in 0..5 {
		bg.replace(0, 0, i % 5 * 100, i % 5 * 100, &images[i as usize % images.len()]);
		bg.check_resized_refresh();
		eprintln!("replace");
		sleep(dur);

	}
}


#[test]
fn test_fade() {
	let (conn, _) = xcb::Connection::connect(None).unwrap();
	let mut bg = XBgSetter::new(&conn).unwrap();
	bg.set_verbose(true);
	let dur = Duration::from_millis(2500);
	for i in 0..5 {
		bg.fade(0, 0, i % 5 * 100, i % 5 * 100, &images[i as usize % images.len()], 5.0);
		bg.check_resized_refresh();
		eprintln!("fade");
		sleep(dur);

	}
}
