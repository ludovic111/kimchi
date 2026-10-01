import "@fontsource-variable/instrument-sans";
import "@fontsource-variable/geist-mono";
import "@fontsource/instrument-serif/400.css";
import "@fontsource/instrument-serif/400-italic.css";
import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";

export default mount(App, { target: document.getElementById("app")! });
