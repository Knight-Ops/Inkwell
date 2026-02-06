use base64::Engine;
use gloo_net::http::Request;
use inkwell_core::ScanResult;
use leptos::*;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::MediaStreamConstraints;

#[component]
pub fn App() -> impl IntoView {
    let video_ref = create_node_ref::<html::Video>();
    let canvas_ref = create_node_ref::<html::Canvas>();
    let (scan_result, set_scan_result) = create_signal::<Option<ScanResult>>(None);
    let (is_scanning, set_is_scanning) = create_signal(false);
    let (camera_error, set_camera_error) = create_signal::<Option<String>>(None);
    let (logs, set_logs) = create_signal::<Vec<String>>(vec![]);
    let (facing_mode, set_facing_mode) = create_signal("environment".to_string());

    // Custom logging helper that goes to console AND screen
    let log_msg = move |msg: String| {
        log::info!("{}", msg);
        set_logs.update(|l| {
            l.push(msg);
            if l.len() > 10 {
                l.remove(0);
            } // Keep last 10
        });
    };

    let log_err = move |msg: String| {
        log::error!("{}", msg);
        set_logs.update(|l| {
            l.push(format!("ERROR: {}", msg));
            if l.len() > 10 {
                l.remove(0);
            }
        });
    };

    // Remove loading message on mount if successful
    create_effect(move |_| {
        if let Some(el) = web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .get_element_by_id("loading_msg")
        {
            el.remove();
        }
        log_msg("App Mounted.".into());

        let window = web_sys::window().unwrap();
        if window.is_secure_context() {
            log_msg("Secure Context: YES".into());
        } else {
            log_err("Secure Context: NO".into());
            log_err("Camera requires HTTPS/localhost".into());
        }
    });

    let (is_torch_on, set_is_torch_on) = create_signal(false);

    let toggle_torch = move |_| {
        let new_state = !is_torch_on.get();
        set_is_torch_on.set(new_state);

        if let Some(video) = video_ref.get() {
            if let Some(stream_val) = video.src_object() {
                let stream = stream_val.unchecked_into::<web_sys::MediaStream>();
                let tracks = stream.get_video_tracks();
                if tracks.length() > 0 {
                    let track = tracks.get(0).unchecked_into::<web_sys::MediaStreamTrack>();

                    // Torch is an advanced constraint
                    let constraints = js_sys::Object::new();
                    let advanced_array = js_sys::Array::new();
                    let torch_obj = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(
                        &torch_obj,
                        &JsValue::from_str("torch"),
                        &JsValue::from_bool(new_state),
                    );
                    advanced_array.push(&torch_obj);
                    let _ = js_sys::Reflect::set(
                        &constraints,
                        &JsValue::from_str("advanced"),
                        &advanced_array,
                    );

                    let _ = track.apply_constraints_with_constraints(
                        &constraints.unchecked_into::<web_sys::MediaTrackConstraints>(),
                    );
                    log_msg(format!(
                        "Flash toggled: {}",
                        if new_state { "ON" } else { "OFF" }
                    ));
                }
            }
        }
    };

    let swap_camera = move |_| {
        set_facing_mode.update(|mode| {
            if mode == "environment" {
                *mode = "user".to_string();
            } else {
                *mode = "environment".to_string();
            }
        });
        set_is_torch_on.set(false); // Reset torch on swap
        log_msg(format!("Swapping camera to: {}", facing_mode.get()));
    };

    // Camera setup and cleanup
    create_effect(move |_| {
        let mode = facing_mode.get();
        if let Some(video) = video_ref.get() {
            spawn_local(async move {
                let window = web_sys::window().expect("no global `window` exists");
                let navigator = window.navigator();

                // 1. Cleanup old stream
                if let Some(old_stream) = video.src_object() {
                    let stream = old_stream.unchecked_into::<web_sys::MediaStream>();
                    log_msg("Stopping old camera tracks...".into());
                    stream.get_tracks().for_each(&mut |track, _index, _array| {
                        track.unchecked_into::<web_sys::MediaStreamTrack>().stop();
                    });
                    video.set_src_object(None);
                }

                log_msg(format!("Requesting camera ({})", mode));

                let media_devices = match navigator.media_devices() {
                    Ok(md) => md,
                    Err(e) => {
                        let err_msg = format!("MediaDevices API Missing/Blocked: {:?}", e);
                        log_err(err_msg.clone());
                        set_camera_error.set(Some(err_msg));
                        return;
                    }
                };

                // Build video constraints for facing mode
                let video_constraints = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &video_constraints,
                    &JsValue::from_str("facingMode"),
                    &JsValue::from_str(&mode),
                );

                let constraints = MediaStreamConstraints::new();
                constraints.set_video(&video_constraints);

                log_msg("Calling getUserMedia...".into());

                match wasm_bindgen_futures::JsFuture::from(
                    media_devices
                        .get_user_media_with_constraints(&constraints)
                        .expect("get_user_media failed"),
                )
                .await
                {
                    Ok(stream) => {
                        let stream = stream.unchecked_into::<web_sys::MediaStream>();
                        log_msg(format!("Camera access granted. Stream ID: {}", stream.id()));
                        video.set_src_object(Some(&stream));
                        video.set_autoplay(true);
                        video.set_muted(true);

                        // Ensure playsinline for iOS
                        video.set_attribute("playsinline", "true").unwrap();

                        let promise = video.play().expect("should return a promise");
                        spawn_local(async move {
                            if let Err(e) = wasm_bindgen_futures::JsFuture::from(promise).await {
                                let err_msg = format!("Video play() failed: {:?}", e);
                                log_err(err_msg.clone());
                                // Only set error if it's not an AbortError (common when swapping quickly)
                                if !format!("{:?}", e).contains("AbortError") {
                                    set_camera_error.set(Some(err_msg));
                                }
                            } else {
                                log_msg("Video playing successfully".into());
                            }
                        });
                    }
                    Err(err) => {
                        let err_msg = format!("Error accessing camera: {:?}", err);
                        log_err(err_msg.clone());
                        set_camera_error.set(Some(err_msg));
                    }
                }
            });
        }
    });

    // The Processing Loop (Recursive RAF)
    create_effect(move |_| {
        let v_ref = video_ref;
        let c_ref = canvas_ref;

        // Define the recursive function using Rc/RefCell for single-threaded WASM
        let f = Rc::new(RefCell::new(None::<Closure<dyn FnMut()>>));
        let f_clone = f.clone();

        let on_frame = move || {
            if let (Some(video), Some(canvas)) = (v_ref.get(), c_ref.get()) {
                let width = video.video_width() as f64;
                let height = video.video_height() as f64;

                if width > 0.0 && height > 0.0 {
                    canvas.set_width(width as u32);
                    canvas.set_height(height as u32);

                    if let Ok(Some(context)) = canvas.get_context("2d") {
                        let context = context.unchecked_into::<web_sys::CanvasRenderingContext2d>();
                        let _ = context.draw_image_with_html_video_element(&video, 0.0, 0.0);
                    }
                }
            }

            // Setup next frame
            let window = web_sys::window().unwrap();
            if let Some(closure) = f_clone.borrow().as_ref() {
                let _ = window.request_animation_frame(closure.as_ref().unchecked_ref());
            }
        };

        // Wrap the closure
        let window = web_sys::window().unwrap();
        let closure = Closure::wrap(Box::new(on_frame) as Box<dyn FnMut()>);
        let _ = window.request_animation_frame(closure.as_ref().unchecked_ref());

        // Store the closure so it stays alive
        f.borrow_mut().replace(closure);
    });

    let start_scan = move |_| {
        set_is_scanning.set(true);
        if let Some(canvas) = canvas_ref.get() {
            // 1. Calculate Crop (Reference area in center)
            let width = canvas.width() as f64;
            let height = canvas.height() as f64;

            // Define a guide box (e.g., 70% of the smaller dimension, roughly card shaped 63:88)
            let box_height = height * 0.7;
            let box_width = box_height * (63.0 / 88.0); // Standard card ratio

            let sx = (width - box_width) / 2.0;
            let sy = (height - box_height) / 2.0;

            // Create a temp canvas for the cropped image
            let document = web_sys::window().unwrap().document().unwrap();
            let crop_canvas = document
                .create_element("canvas")
                .unwrap()
                .unchecked_into::<web_sys::HtmlCanvasElement>();
            crop_canvas.set_width(box_width as u32);
            crop_canvas.set_height(box_height as u32);

            let ctx = crop_canvas
                .get_context("2d")
                .unwrap()
                .unwrap()
                .unchecked_into::<web_sys::CanvasRenderingContext2d>();

            // Draw the middle part of the main canvas onto the crop canvas
            let _ = ctx
                .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                    &canvas, sx, sy, box_width, box_height, 0.0, 0.0, box_width, box_height,
                );

            // Convert crop_canvas to blob/bytes
            let data_url = crop_canvas.to_data_url().unwrap();
            let base64_str = data_url.strip_prefix("data:image/png;base64,").unwrap();

            let bytes = base64::engine::general_purpose::STANDARD
                .decode(base64_str)
                .unwrap();

            spawn_local(async move {
                log_msg(format!("Scanned bytes: {}", bytes.len()));

                // Call API (on port 4000) with raw bytes
                match Request::post("/api/identify")
                    .body(bytes) // Send raw bytes
                    .unwrap()
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let result = resp.json::<ScanResult>().await.unwrap();
                        set_scan_result.set(Some(result));
                    }
                    Err(e) => {
                        log_err(format!("API Request failed: {:?}", e));
                    }
                }
                set_is_scanning.set(false);
            });
        }
    };

    view! {
        <div class="flex flex-col items-center gap-4 p-4 text-white bg-slate-900 min-h-screen relative">
            <h1 class="text-3xl font-bold bg-gradient-to-r from-purple-400 to-pink-600 bg-clip-text text-transparent">
                "Inkwell Scanner"
            </h1>

            <div class="relative rounded-2xl overflow-hidden border-4 border-purple-500 shadow-2xl shadow-purple-500/20 max-w-lg w-full">
                <video
                    node_ref=video_ref
                    autoplay
                    muted
                    playsinline
                    class="w-full h-auto"
                />

                {move || camera_error.get().map(|err| view! {
                    <div class="absolute inset-0 flex items-center justify-center bg-red-900/80 p-4 text-center">
                        <div class="text-white">
                            <p class="font-bold">"Camera Error"</p>
                            <p class="text-xs mt-2">{err}</p>
                            <p class="text-xs mt-4 opacity-50">"Note: Camera requires HTTPS or localhost"</p>
                        </div>
                    </div>
                })}

                // Framing Guide Overlay
                <div class="absolute inset-0 flex items-center justify-center pointer-events-none">
                    <div class="border-2 border-dashed border-white/50 rounded-lg shadow-[0_0_0_9999px_rgba(0,0,0,0.5)] w-[70%] max-w-[300px] aspect-[63/88]">
                        <div class="absolute top-0 left-0 w-4 h-4 border-t-2 border-l-2 border-purple-400 -mt-1 -ml-1"></div>
                        <div class="absolute top-0 right-0 w-4 h-4 border-t-2 border-r-2 border-purple-400 -mt-1 -mr-1"></div>
                        <div class="absolute bottom-0 left-0 w-4 h-4 border-b-2 border-l-2 border-purple-400 -mb-1 -ml-1"></div>
                        <div class="absolute bottom-0 right-0 w-4 h-4 border-b-2 border-r-2 border-purple-400 -mb-1 -mr-1"></div>
                    </div>
                    <div class="absolute top-[10%] text-white text-xs font-bold uppercase tracking-widest opacity-50">
                        "Align card in center"
                    </div>
                </div>

                // Processing canvas (hidden)
                <canvas node_ref=canvas_ref class="hidden" />

                {move || is_scanning.get().then(|| view! {
                    <div class="absolute inset-0 flex items-center justify-center bg-black/50">
                        <div class="animate-spin rounded-full h-16 w-16 border-t-2 border-b-2 border-white"></div>
                    </div>
                })}
            </div>

            <div class="flex gap-4">
                <button
                    on:click=start_scan
                    class="px-8 py-3 bg-purple-600 hover:bg-purple-700 rounded-full font-bold transition-all transform hover:scale-105 shadow-lg shadow-purple-500/20"
                >
                    {move || if is_scanning.get() { "SCANNING..." } else { "SCAN CARD" }}
                </button>

                <button
                    on:click=swap_camera
                    class="p-3 bg-slate-800 hover:bg-slate-700 rounded-full font-bold transition-all transform hover:scale-105 border border-slate-700 shadow-lg"
                    title="Swap Camera"
                >
                    <svg xmlns="http://www.w3.org/2000/svg" class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                    </svg>
                </button>

                <button
                    on:click=toggle_torch
                    class=move || format!(
                        "p-3 rounded-full font-bold transition-all transform hover:scale-105 border shadow-lg {}",
                        if is_torch_on.get() { "bg-yellow-500 border-yellow-400 text-black" } else { "bg-slate-800 border-slate-700 text-white" }
                    )
                    title="Toggle Flash"
                >
                    <svg xmlns="http://www.w3.org/2000/svg" class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z" />
                    </svg>
                </button>
            </div>

            <div class="max-w-lg w-full mt-4">
                {move || scan_result.get().map(|res| {
                    if let Some(card) = res.card {
                        view! {
                            <div class="bg-slate-800 p-6 rounded-2xl border border-slate-700 animate-in fade-in slide-in-from-bottom-4">
                                <h2 class="text-xl font-bold text-purple-400">{card.name}</h2>
                                <p class="text-slate-400 italic">{card.subtitle}</p>
                                <div class="mt-4 flex justify-between items-center">
                                    <span class="text-xs text-slate-500">"Confidence:" {(res.confidence * 100.0).round()}"%"</span>
                                    <span class="px-3 py-1 bg-green-900/50 text-green-400 rounded-full text-sm font-mono uppercase tracking-tighter">
                                        "Found"
                                    </span>
                                </div>
                            </div>
                        }
                    } else {
                        view! {
                            <div class="bg-slate-800 p-6 rounded-2xl border border-slate-700 border-dashed text-center">
                                <p class="text-slate-500">"No match high enough quality. Try again!"</p>
                            </div>
                        }
                    }
                })}
            </div>

            // Debug Logs Overlay
            <div class="fixed bottom-0 left-0 right-0 h-48 bg-black/80 text-green-400 p-4 font-mono text-xs overflow-y-auto border-t border-slate-700 z-50 pointer-events-none">
                <div class="font-bold border-b border-green-900 mb-2">"DEBUG LOGS"</div>
                <ul>
                    {move || logs.get().into_iter().rev().map(|msg| view! { <li>{msg}</li> }).collect_view()}
                </ul>
            </div>
        </div>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount_to_body(App);
}
