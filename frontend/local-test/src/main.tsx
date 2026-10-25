import { createRoot } from "react-dom/client";
import { LocalTest } from "./LocalTest";
import "./style.css";

const root = document.getElementById("root");
if (!root) throw new Error("Local test root is missing.");
createRoot(root).render(<LocalTest />);
