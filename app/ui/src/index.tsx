// OutpostWave UI entry point (SolidJS).
import { render } from "solid-js/web";
import { App } from "./App";

const root = document.getElementById("app");
if (!root) throw new Error("#app root element not found");
render(() => <App />, root);
