use powerline_core::model::Mac;

#[derive(Default)]
pub struct VisualState {
    pub both_dirs: bool,
    pub auto: Option<(Mac, Mac)>,
}
