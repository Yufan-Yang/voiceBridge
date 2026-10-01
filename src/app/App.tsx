import { getCurrentWindow } from "@tauri-apps/api/window";
import { Overlay } from "../components/Overlay";
import { SettingsPanel } from "../components/SettingsPanel";
import { useBackendEvents } from "../hooks/useBackendEvents";

/** Both windows load the same bundle; the window label picks the view. */
export function App() {
  useBackendEvents();
  const label = getCurrentWindow().label;
  document.body.dataset.window = label;
  return label === "settings" ? <SettingsPanel /> : <Overlay />;
}
