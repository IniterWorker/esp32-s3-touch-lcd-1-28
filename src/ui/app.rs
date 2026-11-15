//! The rlvgl `Application`: owns [`AppState`], builds the active
//! page, drains widget actions into `state.apply`, and refreshes
//! live labels on tick.

use alloc::rc::Rc;
use core::cell::RefCell;

use rlvgl_core::WidgetNode;
use rlvgl_core::application::{AppInfo, Application};
use rlvgl_core::event::Event;

use crate::ui::pages::{self, Handles};
use crate::ui::state::AppState;
use crate::ui::{Actions, new_actions};

/// Sandbox application object driven by the bin's UI loop.
pub struct SandboxApp {
    /// Whole application state (settings + telemetry).
    pub state: AppState,
    actions: Actions,
    handles: Handles,
    built_page: u8,
    /// Frame counter (tick count), shown on the hello page.
    pub frame: u32,
}

impl SandboxApp {
    /// New app around the given state.
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            actions: new_actions(),
            handles: Handles::default(),
            built_page: u8::MAX,
            frame: 0,
        }
    }

    fn build_page(&mut self, page: u8) -> WidgetNode {
        self.built_page = page;
        let (root, handles) = pages::build_page(page, &self.state, &self.actions);
        self.handles = handles;
        root
    }

    /// Drain queued widget actions into the state. Returns true if
    /// any action was applied.
    fn drain_actions(&mut self) -> bool {
        let pending: alloc::vec::Vec<(&'static str, alloc::string::String)> =
            self.actions.borrow_mut().drain(..).collect();
        let mut any = false;
        for (k, v) in pending {
            if self.state.apply(k, &v).is_ok() {
                any = true;
            }
        }
        any
    }
}

impl Application for SandboxApp {
    fn info(&self) -> AppInfo {
        AppInfo {
            name: "SBX sandbox",
            version: "0.1.0",
            preferred_width: 240,
            preferred_height: 240,
        }
    }

    fn build(&mut self, _width: u32, _height: u32) -> WidgetNode {
        let page = self.state.page;
        self.build_page(page)
    }

    fn after_event(&mut self, root: &Rc<RefCell<WidgetNode>>, _event: &Event) {
        self.drain_actions();
        if self.state.page != self.built_page {
            let page = self.state.page;
            *root.borrow_mut() = self.build_page(page);
        }
    }

    fn tick(&mut self, root: &Rc<RefCell<WidgetNode>>) {
        self.frame = self.frame.wrapping_add(1);
        self.drain_actions();
        if self.state.page != self.built_page {
            let page = self.state.page;
            *root.borrow_mut() = self.build_page(page);
        }
        pages::refresh(&self.handles, &self.state, self.frame);
        // Control widgets (slider/switches) two-way sync with state.
        pages::sync_controls(&self.handles, &mut self.state);
    }
}
