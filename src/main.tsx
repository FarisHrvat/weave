import { render } from "solid-js/web";
import App from "./App";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";

render(() => <App />, document.getElementById("root")!);
