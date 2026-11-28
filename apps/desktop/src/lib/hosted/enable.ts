/**
 * Whether the page can run the Rust engine hosted in the Tauri shell. The packaged app always can. A plain browser has no shell to
 * host the engine, so there the app shows a message instead (there is no other engine).
 */
import { isTauri } from '../../config';

export function hostedEngineAvailable(): boolean {
  return isTauri();
}

export function hostedEngineEnabled(): boolean {
  return hostedEngineAvailable();
}
