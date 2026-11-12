use std::collections::HashMap;

pub struct Widget {
    name: String,
}

pub enum Mode {
    Fast,
    Slow,
}

pub trait Render {
    fn render(&self);
}

impl Widget {
    pub fn new(name: String) -> Self {
        Self { name }
    }

    pub fn render(&self) {
        helper();
    }
}

pub const MAX: usize = 10;

pub fn helper() -> usize {
    MAX
}

fn main() {
    let mut map: HashMap<String, usize> = HashMap::new();
    map.insert("a".to_owned(), helper());
    let widget = Widget::new("x".to_owned());
    widget.render();
}
