use base64::Engine;
use gloo_net::http::Request;
use inkwell_core::ScanResult;
use leptos::*;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::MediaStreamConstraints;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct LorcastPrices {
    pub usd: Option<String>,
    pub usd_foil: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct LorcastCard {
    pub prices: LorcastPrices,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CsvFormat {
    Standard,
    Dreamborn,
}

#[derive(Clone, Debug)]
pub struct ScannedItem {
    pub card: inkwell_core::Card,
    pub prices: Option<LorcastPrices>,
    pub is_foil: bool,
    pub scanned_at: String,
}

#[component]
pub fn App() -> impl IntoView {
    let video_ref = create_node_ref::<html::Video>();
    let canvas_ref = create_node_ref::<html::Canvas>();
    let (scan_result, set_scan_result) = create_signal::<Option<ScanResult>>(None);
    let (is_scanning, set_is_scanning) = create_signal(false);
    let (camera_error, set_camera_error) = create_signal::<Option<String>>(None);
    let (logs, set_logs) = create_signal::<Vec<String>>(vec![]);
    let (facing_mode, set_facing_mode) = create_signal("environment".to_string());
    let (scanned_cards, set_scanned_cards) = create_signal::<Vec<ScannedItem>>(vec![]);
    let (show_logs, set_show_logs) = create_signal(false);
    let (global_total, set_global_total) = create_signal(0u64);
    let (csv_format, set_csv_format) = create_signal(CsvFormat::Standard);
    let (scan_status, set_scan_status) = create_signal::<Option<bool>>(None);

    let running_total = move || {
        scanned_cards.get().iter().fold(0.0, |acc, item| {
            let price_str = item
                .prices
                .as_ref()
                .and_then(|p| {
                    if item.is_foil {
                        p.usd_foil.as_deref().or(p.usd.as_deref())
                    } else {
                        p.usd.as_deref()
                    }
                })
                .unwrap_or("0");
            let val: f64 = price_str.parse().unwrap_or(0.0);
            acc + val
        })
    };

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

        // Fetch initial stats and poll every 5 seconds
        let fetch_stats = move || {
            spawn_local(async move {
                if let Ok(resp) = Request::get("/api/stats").send().await {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(total) = json["total_scanned_cards"].as_u64() {
                            set_global_total.set(total);
                        }
                    }
                }
            });
        };

        fetch_stats(); // First fetch
        let interval = gloo_timers::callback::Interval::new(5000, fetch_stats);
        on_cleanup(move || {
            drop(interval);
        });
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

                // Cleanup old stream
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
            // Calculate Crop (Reference area in center)
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
                        if let Some(card) = result.card.clone() {
                            set_scan_status.set(Some(true));
                            let lorcast_url = format!(
                                "https://api.lorcast.com/v0/cards/{}/{}",
                                card.set_code, card.card_number
                            );
                            let prices =
                                match gloo_net::http::Request::get(&lorcast_url).send().await {
                                    Ok(res) if res.ok() => {
                                        res.json::<LorcastCard>().await.ok().map(|lc| lc.prices)
                                    }
                                    _ => None,
                                };
                            let scanned_at = js_sys::Date::new_0()
                                .to_iso_string()
                                .as_string()
                                .unwrap_or_default();

                            let item = ScannedItem {
                                card,
                                prices,
                                is_foil: false,
                                scanned_at,
                            };
                            set_scanned_cards.update(|list| list.push(item));
                        } else {
                            set_scan_status.set(Some(false));
                        }
                        set_timeout(
                            move || set_scan_status.set(None),
                            std::time::Duration::from_millis(1500),
                        );
                        set_global_total.set(result.global_total_scans);
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

    let download_csv = move |_| {
        let cards = scanned_cards.get();
        if cards.is_empty() {
            log_err("No cards to export!".into());
            return;
        }

        let csv_content = generate_csv(&cards, csv_format.get());

        // Trigger download
        let window = web_sys::window().unwrap();
        let document = window.document().unwrap();
        let body = document.body().unwrap();

        let blob_parts = js_sys::Array::new();
        blob_parts.push(&JsValue::from_str(&csv_content));

        let blob_props = web_sys::BlobPropertyBag::new();
        blob_props.set_type("text/csv");

        let blob =
            web_sys::Blob::new_with_str_sequence_and_options(&blob_parts, &blob_props).unwrap();
        let url = web_sys::Url::create_object_url_with_blob(&blob).unwrap();

        let anchor = document
            .create_element("a")
            .unwrap()
            .unchecked_into::<web_sys::HtmlAnchorElement>();
        anchor.set_href(&url);
        anchor.set_download("inkwell_matches.csv");
        anchor.style().set_property("display", "none").unwrap();
        body.append_child(&anchor).unwrap();
        anchor.click();
        body.remove_child(&anchor).unwrap();
        web_sys::Url::revoke_object_url(&url).unwrap();

        log_msg("CSV Download triggered.".into());
    };

    let reset_session = move |_| {
        set_scanned_cards.set(vec![]);
        set_scan_result.set(None);
        log_msg("Session reset.".into());
    };

    view! {
        <div class="flex flex-col items-center gap-3 sm:gap-4 p-2 sm:p-4 text-white bg-slate-900 min-h-screen relative pb-16">
            <h1 class="text-2xl sm:text-3xl font-bold bg-gradient-to-r from-purple-400 to-pink-600 bg-clip-text text-transparent pt-2">
                "Inkwell Scanner"
            </h1>

            <div class="text-lg sm:text-xl font-bold text-emerald-400 bg-slate-800 px-4 sm:px-6 py-2 rounded-full border border-slate-700 shadow-lg mb-1 sm:mb-2 text-center">
                "Session Total: $"
                {move || format!("{:.2}", running_total())}
                <span class="text-xs text-slate-500 ml-2">{move || format!("({} cards)", scanned_cards.get().len())}</span>
            </div>

            <div class="text-xs sm:text-sm font-bold text-purple-300 bg-slate-800/50 px-4 py-1 rounded-full border border-slate-700 tracking-wide mb-2 sm:mb-4 text-center">
                "Global Scans: "
                <span class="text-white">{move || global_total.get()}</span>
            </div>

            <div class=move || format!(
                "relative rounded-2xl overflow-hidden border-4 shadow-2xl max-w-lg w-full transition-colors duration-500 {}",
                match scan_status.get() {
                    Some(true) => "border-green-500 shadow-green-500/50",
                    Some(false) => "border-red-500 shadow-red-500/50",
                    None => "border-purple-500 shadow-purple-500/20",
                }
            )>
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

            <div class="flex flex-col sm:flex-row w-full max-w-lg gap-3 mt-2 px-2 sm:px-0">
                <button
                    on:click=start_scan
                    class="w-full sm:w-auto sm:flex-1 px-8 py-4 sm:py-3 bg-purple-600 hover:bg-purple-700 rounded-2xl sm:rounded-full font-bold transition-all transform hover:scale-105 shadow-lg shadow-purple-500/20 text-xl sm:text-base tracking-wide"
                >
                    {move || if is_scanning.get() { "SCANNING..." } else { "SCAN CARD" }}
                </button>

                <div class="flex flex-wrap sm:flex-nowrap w-full sm:w-auto gap-2 sm:gap-4 justify-between">
                    <div class="w-full sm:w-auto flex flex-row items-stretch overflow-hidden rounded-2xl sm:rounded-full shadow-lg shadow-emerald-500/20 transform transition-all hover:scale-105 relative z-40">
                        <select
                            on:change=move |ev| {
                                let val = event_target_value(&ev);
                                set_csv_format.set(if val == "dreamborn" { CsvFormat::Dreamborn } else { CsvFormat::Standard });
                            }
                            class="flex-1 sm:flex-none py-3 sm:py-2 px-3 sm:px-2 bg-emerald-700 text-white border-r border-emerald-600 outline-none text-xs font-bold text-center cursor-pointer appearance-none min-w-[80px]"
                        >
                            <option value="standard">"Standard CSV"</option>
                            <option value="dreamborn">"Dreamborn CSV"</option>
                        </select>
                        <button
                            on:click=download_csv
                            class="flex-none px-6 sm:px-4 flex flex-row justify-center items-center gap-2 py-3 sm:py-2 bg-emerald-600 hover:bg-emerald-500 font-bold transition-all"
                        >
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5 sm:w-5 sm:h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
                            </svg>
                            <span class="text-[10px] sm:text-base uppercase whitespace-nowrap">"CSV"</span>
                        </button>
                    </div>

                    <button
                        on:click=swap_camera
                        class="flex-1 sm:flex-none flex justify-center items-center py-2 sm:p-3 bg-slate-800 hover:bg-slate-700 rounded-2xl sm:rounded-full font-bold transition-all transform hover:scale-105 border border-slate-700 shadow-lg"
                        title="Swap Camera"
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                        </svg>
                    </button>

                    <button
                        on:click=toggle_torch
                        class=move || format!(
                            "flex-1 sm:flex-none flex justify-center items-center py-2 sm:p-3 rounded-2xl sm:rounded-full font-bold transition-all transform hover:scale-105 border shadow-lg {}",
                            if is_torch_on.get() { "bg-yellow-500 border-yellow-400 text-black" } else { "bg-slate-800 border-slate-700 text-white" }
                        )
                        title="Toggle Flash"
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z" />
                        </svg>
                    </button>

                    <button
                        on:click=reset_session
                        class="flex-1 sm:flex-none flex justify-center items-center py-2 sm:p-3 bg-red-900/50 hover:bg-red-800 text-red-500 hover:text-red-400 rounded-2xl sm:rounded-full font-bold transition-all transform hover:scale-105 border border-red-900/50 shadow-lg"
                        title="Reset Session"
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                        </svg>
                    </button>
                </div>
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
                                {
                                    let items = scanned_cards.get();
                                    if let Some(last_item) = items.last().cloned() {
                                        if last_item.card.phash == card.phash && last_item.card.id == card.id {
                                            let price_usd = last_item.prices.as_ref().and_then(|p| p.usd.clone()).unwrap_or_else(|| "N/A".to_string());
                                            let price_foil = last_item.prices.as_ref().and_then(|p| p.usd_foil.clone()).unwrap_or_else(|| "N/A".to_string());
                                            let is_currently_foil = last_item.is_foil;

                                            let toggle_foil = move |_| {
                                                set_scanned_cards.update(|list| {
                                                    if let Some(last_mut) = list.last_mut() {
                                                        last_mut.is_foil = !last_mut.is_foil;
                                                    }
                                                });
                                            };

                                            view! {
                                                <div class="mt-4 border-t border-slate-700 pt-4">
                                                    <div class="flex justify-between items-center text-sm font-mono mb-2">
                                                        <span class="text-slate-400">"Normal: $" {price_usd}</span>
                                                        <span class="text-purple-400">"Foil: $" {price_foil}</span>
                                                    </div>
                                                    <label class="flex items-center gap-2 cursor-pointer mt-2 text-sm text-slate-300 w-max">
                                                        <input
                                                            type="checkbox"
                                                            class="w-4 h-4 rounded border-slate-600 text-purple-600 focus:ring-purple-600 bg-slate-700"
                                                            on:change=toggle_foil
                                                            checked=is_currently_foil
                                                        />
                                                        "Mark as Foil"
                                                    </label>
                                                </div>
                                            }.into_view()
                                        } else {
                                            ().into_view()
                                        }
                                    } else {
                                        ().into_view()
                                    }
                                }
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
            <div class=move || format!(
                "fixed bottom-0 left-0 right-0 bg-black/95 text-green-400 font-mono text-xs transition-all duration-500 z-50 flex flex-col border-t border-slate-700 transform {}",
                if show_logs.get() { "h-64 translate-y-0" } else { "h-64 translate-y-full" }
            )>
                <div
                    on:click=move |_| set_show_logs.set(false)
                    class="flex items-center justify-between px-4 min-h-[40px] cursor-pointer border-b border-green-900/30 bg-black/50 hover:bg-black/20 pointer-events-auto"
                >
                    <div class="flex items-center gap-2">
                        <div class=move || format!("w-2 h-2 rounded-full bg-green-500 {}", if is_scanning.get() { "animate-pulse" } else { "" })></div>
                        <span class="font-bold uppercase tracking-tighter">"System Logs"</span>
                    </div>
                    <span class="text-[10px] opacity-50 uppercase tracking-widest">"Hide"</span>
                </div>
                <div class="p-4 overflow-y-auto flex-1 pointer-events-auto">
                    <ul>
                        {move || logs.get().into_iter().rev().map(|msg| view! { <li>{msg}</li> }).collect_view()}
                    </ul>
                </div>
            </div>

            // Small Floating Toggle Button
            <button
                on:click=move |_| set_show_logs.set(true)
                class=move || format!(
                    "fixed bottom-16 right-4 bg-black/80 text-green-500 px-3 py-1.5 rounded-lg border border-green-900/50 text-[10px] uppercase font-mono z-40 hover:bg-black transition-all duration-300 transform shadow-lg {}",
                    if show_logs.get() { "translate-y-20 opacity-0 pointer-events-none" } else { "translate-y-0 opacity-100 pointer-events-auto" }
                )
            >
                "Show Logs"
            </button>
        </div>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount_to_body(App);
}

pub fn generate_csv(items: &[ScannedItem], format: CsvFormat) -> String {
    match format {
        CsvFormat::Dreamborn => {
            let mut csv = String::from("Set Number,Card Number,Variant,Count\n");

            let mut rows: Vec<((String, u32, bool), usize)> = Vec::new();

            for item in items {
                let card = &item.card;

                let group_key = card
                    .promo_grouping
                    .clone()
                    .unwrap_or_else(|| format!("0{}", card.set_code));

                let key = (group_key.clone(), card.card_number, item.is_foil);

                if let Some(pos) = rows.iter().position(|row| row.0 == key) {
                    rows[pos].1 += 1;
                } else {
                    rows.push((key, 1));
                }
            }

            for (key, count) in rows {
                let variant = if key.2 { "foil" } else { "normal" };
                csv.push_str(&format!("{},{},{},{}\n", key.0, key.1, variant, count));
            }

            csv
        }
        CsvFormat::Standard => {
            let mut csv = String::from(
                "Set Number,Card Number,Variant,Count,Card Name,Rarity,Price,ScannedAt\n",
            );

            let mut rows: Vec<((String, u32, bool), usize, String, String, String, String)> =
                Vec::new();

            for item in items {
                let card = &item.card;
                let full_name = if card.subtitle.is_empty() {
                    card.name.clone()
                } else {
                    format!("{} - {}", card.name, card.subtitle)
                };

                let escaped_name = if full_name.contains(',') || full_name.contains('"') {
                    format!("\"{}\"", full_name.replace('"', "\"\""))
                } else {
                    full_name
                };

                let price_str = item
                    .prices
                    .as_ref()
                    .and_then(|p| {
                        if item.is_foil {
                            p.usd_foil.as_deref().or(p.usd.as_deref())
                        } else {
                            p.usd.as_deref()
                        }
                    })
                    .unwrap_or("0");

                let group_key = card
                    .promo_grouping
                    .clone()
                    .unwrap_or_else(|| card.set_code.clone());

                let key = (group_key.clone(), card.card_number, item.is_foil);

                if let Some(pos) = rows.iter().position(|row| row.0 == key) {
                    rows[pos].1 += 1;
                } else {
                    rows.push((
                        key,
                        1,
                        escaped_name,
                        card.rarity.clone(),
                        price_str.to_string(),
                        item.scanned_at.clone(),
                    ));
                }
            }

            for (key, count, name, rarity, price, scanned_at) in rows {
                let variant = if key.2 { "foil" } else { "normal" };
                csv.push_str(&format!(
                    "{},{},{},{},{},{},{},{}\n",
                    key.0, key.1, variant, count, name, rarity, price, scanned_at
                ));
            }

            csv
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkwell_core::Card;

    #[test]
    fn test_csv_generation() {
        let items = vec![
            ScannedItem {
                card: Card {
                    id: "1".into(),
                    name: "Mickey Mouse".into(),
                    subtitle: "Wayward Sorcerer".into(),
                    phash: "".into(),
                    akaze_data: vec![],
                    image_url: "".into(),
                    rarity: "Common".into(),
                    promo_grouping: None,
                    set_code: "1".into(),
                    card_number: 123,
                },
                prices: Some(LorcastPrices {
                    usd: Some("1.50".into()),
                    usd_foil: Some("5.00".into()),
                }),
                is_foil: false,
                scanned_at: "2026-02-23T21:55:00.000Z".into(),
            },
            ScannedItem {
                card: Card {
                    id: "1".into(), // Duplicate item
                    name: "Mickey Mouse".into(),
                    subtitle: "Wayward Sorcerer".into(),
                    phash: "".into(),
                    akaze_data: vec![],
                    image_url: "".into(),
                    rarity: "Common".into(),
                    promo_grouping: None,
                    set_code: "1".into(),
                    card_number: 123,
                },
                prices: None,
                is_foil: false,
                scanned_at: "2026-02-23T21:56:00.000Z".into(),
            },
            ScannedItem {
                card: Card {
                    id: "2".into(),
                    name: "Donald Duck, The Brave".into(), // contains comma
                    subtitle: "".into(),
                    phash: "".into(),
                    akaze_data: vec![],
                    image_url: "".into(),
                    rarity: "Rare".into(),
                    promo_grouping: Some("P3".into()),
                    set_code: "6".into(),
                    card_number: 45,
                },
                prices: Some(LorcastPrices {
                    usd: Some("2.00".into()),
                    usd_foil: None,
                }), // missing foil price fallback
                is_foil: true,
                scanned_at: "2026-02-23T21:56:00.000Z".into(),
            },
        ];

        let csv_standard = generate_csv(&items, CsvFormat::Standard);
        let lines_std: Vec<&str> = csv_standard.lines().collect();
        assert_eq!(lines_std.len(), 3);
        assert_eq!(
            lines_std[0],
            "Set Number,Card Number,Variant,Count,Card Name,Rarity,Price,ScannedAt"
        );
        assert_eq!(
            lines_std[1],
            "1,123,normal,2,Mickey Mouse - Wayward Sorcerer,Common,1.50,2026-02-23T21:55:00.000Z"
        );
        assert_eq!(
            lines_std[2],
            "P3,45,foil,1,\"Donald Duck, The Brave\",Rare,2.00,2026-02-23T21:56:00.000Z"
        );

        let csv_dream = generate_csv(&items, CsvFormat::Dreamborn);
        let lines_dream: Vec<&str> = csv_dream.lines().collect();
        assert_eq!(lines_dream.len(), 3);
        assert_eq!(lines_dream[0], "Set Number,Card Number,Variant,Count");
        assert_eq!(lines_dream[1], "01,123,normal,2");
        assert_eq!(lines_dream[2], "P3,45,foil,1");
    }
}
