//! The preview, made on a thread of its own so dragging a slider never
//! waits for pixels. Only the newest request matters: one asked for while
//! another renders replaces any that waited. Each is made at exactly the
//! size it's shown, so the screen copies it one to one.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use egui::{Color32, ColorImage};

use crate::develop::{self, View};
use crate::image::Photo;
use crate::project::Edit;

#[derive(Clone)]
pub struct Request {
    pub photo: Arc<Photo>,
    pub edit: Edit,
    pub view: View,
}

impl PartialEq for Request {
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.photo, &o.photo) && self.edit == o.edit && self.view == o.view
    }
}

/// A finished preview.
pub struct Done {
    pub image: ColorImage,
    pub view: View,
    /// Counts of each level of red, green, blue and brightness.
    pub histogram: Box<[[u32; 256]; 4]>,
    pub ms: f32,
}

#[derive(Default)]
struct Slots {
    next: Option<Request>,
    done: Option<Done>,
    quit: bool,
}

pub struct Preview {
    shared: Arc<(Mutex<Slots>, Condvar)>,
    /// The last request sent, so the same one isn't sent twice.
    pub last: Option<Request>,
}

impl Preview {
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let shared: Arc<(Mutex<Slots>, Condvar)> = Arc::default();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("preview".into())
            .spawn(move || {
                loop {
                    let req = {
                        let (lock, cv) = &*s;
                        let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if g.quit {
                                return;
                            }
                            if let Some(r) = g.next.take() {
                                break r;
                            }
                            g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    let done = make(&req);
                    s.0.lock().unwrap_or_else(|e| e.into_inner()).done = Some(done);
                    wake();
                }
            })
            .expect("a thread");
        Self { shared, last: None }
    }

    /// Asks for `req`, unless it was the last one asked for.
    pub fn ask(&mut self, req: Request) {
        if self.last.as_ref() == Some(&req) {
            return;
        }
        self.last = Some(req.clone());
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).next = Some(req);
        cv.notify_one();
    }

    /// Forget the last request, so the next one is made even if it's the same.
    pub fn again(&mut self) {
        self.last = None;
    }

    pub fn take(&self) -> Option<Done> {
        self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).done.take()
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).quit = true;
        cv.notify_one();
    }
}

fn make(req: &Request) -> Done {
    let t = Instant::now();
    let v = req.view;
    let px = develop::render(&req.photo, &req.edit, &v, 0, v.h, develop::to_u8);
    let mut histogram = Box::new([[0u32; 256]; 4]);
    let mut pixels = Vec::with_capacity(v.w * v.h);
    for p in px.as_chunks::<4>().0 {
        if p[3] > 0 {
            histogram[0][p[0] as usize] += 1;
            histogram[1][p[1] as usize] += 1;
            histogram[2][p[2] as usize] += 1;
            let y = (u32::from(p[0]) * 54 + u32::from(p[1]) * 183 + u32::from(p[2]) * 19) >> 8;
            histogram[3][y.min(255) as usize] += 1;
        }
        pixels.push(Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]));
    }
    Done { image: ColorImage::new([v.w, v.h], pixels), view: v, histogram, ms: t.elapsed().as_secs_f32() * 1000.0 }
}
