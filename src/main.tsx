import { render } from "solid-js/web";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@xterm/xterm/css/xterm.css";
import "./styles/tokens.css";
import "./styles/app.css";
import "./styles/layout.css";
import "./styles/screens.css";
import { App } from "./App";
import { init } from "./store/app";
import { isMac, isTauri } from "./lib/ipc";

if (isTauri && isMac) document.documentElement.classList.add("vibrant");

// Paint the window shell first; the snapshot fills it in when the backend answers.
render(() => <App />, document.getElementById("root")!);
void init();
