import { createRoot } from "react-dom/client";
import { LocalTest } from "./LocalTest";
import { LibraryTest } from "./LibraryTest";
import { ProviderTest } from "./ProviderTest";
import { ArtistPhotoTest } from "./ArtistPhotoTest";
import "../../app/styles.css";
import "./style.css";

const root = document.getElementById("root");
if (!root) throw new Error("Local test root is missing.");
createRoot(root).render(
  window.location.search === "?artist-photos=1" ? (
    <ArtistPhotoTest />
  ) : window.location.search === "?provider=1" ? (
    <ProviderTest />
  ) : window.location.search === "?library=1" ? (
    <LibraryTest />
  ) : (
    <LocalTest />
  ),
);
