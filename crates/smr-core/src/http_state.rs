use std::sync::Arc;

use crate::proxy::ProxyService;
use crate::state::SharedApp;

#[derive(Clone)]
pub struct HttpState {
    pub app: Arc<SharedApp>,
    pub proxy: Arc<ProxyService>,
}
