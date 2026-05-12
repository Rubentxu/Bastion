//! Root App component for the Bastion dashboard UI.
//!
//! This is a minimal working version of the Leptos WASM application.

use leptos::{component, view, IntoView};
use leptos::prelude::{ElementChild, ClassAttribute};

/// Initialize logging for WASM.
pub fn init_logging() {
    console_log::init_with_level(log::Level::Info).ok();
}

/// Root App component.
#[component]
pub fn App() -> impl IntoView {
    init_logging();

    view! {
        <div class="flex h-screen bg-gray-100">
            <nav class="w-64 bg-gray-900 text-white">
                <div class="p-4 border-b border-gray-700">
                    <h1 class="text-xl font-bold">Bastion</h1>
                    <p class="text-sm text-gray-400">Dashboard</p>
                </div>
                <div class="p-4">
                    <h2 class="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-2">
                        Projects
                    </h2>
                    <p class="text-gray-400 text-sm">No projects yet</p>
                </div>
            </nav>
            <main class="flex-1 p-6">
                <h1 class="text-2xl font-bold text-gray-900">Welcome to Bastion Dashboard</h1>
                <p class="mt-1 text-sm text-gray-500">
                    Select a project to view its sandboxes and details.
                </p>
            </main>
        </div>
    }
}
