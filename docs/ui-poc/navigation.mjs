const subnavigation = () => `<nav class="navigation core-pages" aria-label="Music management pages">
  <a href="#music" aria-current="page">Music</a>
  <a href="#music-library">Library</a>
  <a href="#music-queue">Queue</a>
</nav>`;
const globalNavigation = () => `<nav class="navigation app-pages" aria-label="Application pages">
  <a href="#inbox">Inbox</a><a href="#recovery">Recovery</a><a href="#settings">App settings</a>
</nav>`;
const futurePages = (name) => `<span class="future-caption">Future module / navigation example</span>
  <button disabled>${name}</button><button disabled>Library</button><button disabled>Queue</button>`;

export function navigationStudy(html) {
  const navigation = `<aside class="sidebar core-sidebar">
    <div class="core-variant switcher-variant">
      <a class="wordmark" href="#music" aria-label="Domus Obscura">DOMUS <span>OBSCURA</span></a>
      <label class="core-picker">Core function<select aria-label="Core function"><option>Music management</option><option disabled>TV / future</option><option disabled>Movies / future</option></select></label>
      ${subnavigation()}${globalNavigation()}
      <p class="core-note">Music has its own Library and Queue.</p>
    </div>
    <div class="core-variant rail-variant">
      <nav class="primary-rail" aria-label="Core functions">
        <a class="rail-brand" href="#music" aria-label="Domus Obscura">DO</a>
        <button aria-current="true" aria-label="Music management core"><span aria-hidden="true">&#9835;</span>Music</button>
        <button disabled aria-label="TV core, future"><span aria-hidden="true">&#9635;</span>TV<small>Future</small></button>
        <button disabled aria-label="Movies core, future"><span aria-hidden="true">&#9641;</span>Movies<small>Future</small></button>
      </nav>
      <div class="secondary-rail"><h2>MUSIC<br>MANAGEMENT</h2>${subnavigation()}${globalNavigation()}<p class="core-note">The core rail stays visible when its pages change.</p></div>
    </div>
    <div class="core-variant tree-variant">
      <a class="wordmark" href="#music" aria-label="Domus Obscura">DOMUS <span>OBSCURA</span></a>
      <div class="core-tree">
        <details open><summary>Music management</summary>${subnavigation()}</details>
        <details><summary>TV <span class="future-tag">Future</span></summary><div class="future-pages">${futurePages('TV')}</div></details>
        <details><summary>Movies <span class="future-tag">Future</span></summary><div class="future-pages">${futurePages('Movies')}</div></details>
      </div>
      ${globalNavigation()}<p class="core-note">Each group owns its own pages and workflows.</p>
    </div>
  </aside>`;
  const styles = `
.core-sidebar{display:block;padding:0;align-self:stretch}.core-variant{display:none;min-height:100%;padding:25px 12px}.core-variant .wordmark{display:block;margin-bottom:29px}
.shell[data-core-nav=switcher]{grid-template-columns:202px minmax(0,1fr)}.shell[data-core-nav=switcher] .switcher-variant{display:flex;flex-direction:column}
.core-picker{display:block;font-size:12px;color:#c6b8d5;margin-bottom:24px}.core-picker select{display:block;width:100%;padding:9px 8px;margin-top:8px;border:1px solid #88749f;border-radius:5px;background:#252032;color:#f2e8ff;font-size:13px}
.core-pages{gap:5px}.app-pages{margin-top:auto;border-top:1px solid #493b55;padding-top:15px}.core-note{font-size:12px;color:#bdadcd;padding:12px 10px 0;margin-bottom:0;max-width:27ch}
.shell[data-core-nav=rail]{grid-template-columns:226px minmax(0,1fr)}.shell[data-core-nav=rail] .rail-variant{display:grid;grid-template-columns:62px minmax(0,1fr);padding:0}
.primary-rail{border-right:1px solid #403448;background:#16111f;padding:21px 5px;display:flex;flex-direction:column;gap:12px}.primary-rail button{display:flex;flex-direction:column;align-items:center;justify-content:center;padding:9px 2px;font-size:11px;gap:3px;min-height:68px;width:100%;border-color:transparent;background:transparent;border-radius:5px}.primary-rail button[aria-current]{background:#38244f;color:#ebd7ff;border-color:#9d77c8}.primary-rail button span{font-size:23px;line-height:1.2}.primary-rail button:disabled{color:#b9aec6;background:transparent}.primary-rail small{font-size:10px}.rail-brand{font-family:Poster,sans-serif;font-size:28px;text-align:center;color:#ddc4ff;margin-bottom:20px;display:grid;place-items:center}
.secondary-rail{padding:26px 10px;display:flex;flex-direction:column}.secondary-rail h2{font-size:24px;line-height:1.15;color:#ebdefa;margin:0 8px 27px}.secondary-rail .core-note{padding-left:8px}.secondary-rail .navigation a{padding-left:9px;font-size:13px}
.shell[data-core-nav=tree]{grid-template-columns:216px minmax(0,1fr)}.shell[data-core-nav=tree] .tree-variant{display:flex;flex-direction:column}
.core-tree details{border-top:1px solid #39303f;padding:5px 0}.core-tree summary{min-height:48px;cursor:pointer;padding:12px 7px;color:#f1e7fb;font-size:14px}.core-tree details[open]>summary{color:#d4b4ff}.core-tree summary::marker{color:#c49eff}.core-tree .core-pages{padding:0 0 13px 13px}.future-tag{font-size:10px;margin-left:7px;color:#c8b8d7;border:1px solid #62516f;border-radius:3px;padding:2px 4px}.future-pages{display:grid;padding:5px 13px 15px;gap:4px}.future-pages button{background:transparent;border:0;text-align:left;color:#b6a9c3;font-size:13px;min-height:36px}.future-caption{font-size:11px;color:#c4b2d6;margin-bottom:7px}
@container(max-width:1100px){.shell[data-core-nav=switcher]{grid-template-columns:190px minmax(0,1fr)}.shell[data-core-nav=rail]{grid-template-columns:212px minmax(0,1fr)}.shell[data-core-nav=tree]{grid-template-columns:200px minmax(0,1fr)}.core-variant{padding-left:10px;padding-right:10px}}
@container(max-width:800px){.shell[data-core-nav]{grid-template-columns:1fr}.core-sidebar{border-right:0;border-bottom:1px solid #403448;padding:0}.core-variant{min-height:0}.switcher-variant{padding:16px 18px!important;display:none;flex-direction:row!important;align-items:center;flex-wrap:wrap;gap:13px 20px}.core-variant .wordmark{font-size:24px;margin-bottom:0}.core-picker{margin:0;font-size:11px;flex:1;min-width:155px}.core-picker select{margin-top:3px}.core-pages{width:100%;display:flex}.core-pages a{min-height:44px}.app-pages,.core-note{display:none}.rail-variant{grid-template-columns:64px minmax(0,1fr)!important}.primary-rail{padding:12px 4px;gap:8px}.primary-rail button{min-height:52px}.primary-rail button span{display:none}.primary-rail .rail-brand{font-size:24px;margin-bottom:4px;min-height:30px}.secondary-rail{justify-content:center;padding:18px}.secondary-rail h2{font-size:24px;margin:0 8px 12px}.secondary-rail h2 br{display:none}.secondary-rail .core-pages{gap:8px;flex-wrap:wrap}.tree-variant{padding:16px 18px}.tree-variant .wordmark{margin-bottom:14px}.core-tree{display:flex;gap:13px;align-items:start;flex-wrap:wrap}.core-tree details{flex:1;min-width:130px}.core-tree details:first-child{flex-basis:100%}.core-tree .core-pages{padding-bottom:0}.tree-variant .future-pages{padding-left:0}}
`;
  const script = `
const shell=document.querySelector('.shell');
const navigationDescriptions={
 switcher:'Core switcher: choose a core, then work within its own Music, Library and Queue pages. This leaves the most room for artwork and review.',
 rail:'Two-level rail: core functions stay on the outer rail; the active core has a separate page menu. This makes switching cores more direct.',
 tree:'Grouped navigation: each core is an expandable group with its own pages. The full hierarchy is visible without opening a switcher.',
};
function setNavigation(mode){
 shell.dataset.coreNav=mode;
 document.querySelectorAll('[data-core-option]').forEach(button=>button.setAttribute('aria-pressed',String(button.dataset.coreOption===mode)));
 document.querySelector('#description').textContent=navigationDescriptions[mode];
}
document.addEventListener('click',event=>{
 const option=event.target.closest('[data-core-option]');
 if(option){setNavigation(option.dataset.coreOption);document.querySelector('#feedback').textContent='Navigation study only. Music is the active core. TV and Movies have no implementation in this preview.';}
});
document.addEventListener('input',()=>setNavigation(shell.dataset.coreNav));
document.querySelector('#feedback').textContent='Music management contains Music, Library and Queue. TV and Movies are future core functions, not enabled features.';
setNavigation('switcher');
`;
  return html
    .replace('Domus Obscura - Cover wall layout studies', 'Domus Obscura - core navigation studies')
    .replace('Generated sample albums. No provider, approval or download access.', 'Generated sample music. TV and Movies are future navigation examples only.')
    .replace(/<div class="study-controls"[\s\S]*?<\/div>/, `<div class="study-controls" aria-label="Core navigation studies">
      <button data-core-option="switcher" aria-pressed="true">Core switcher</button>
      <button data-core-option="rail" aria-pressed="false">Two-level rail</button>
      <button data-core-option="tree" aria-pressed="false">Grouped navigation</button>
      <button class="narrow-toggle" aria-pressed="false">Narrow view</button></div>`)
    .replace(/<aside class="sidebar">[\s\S]*?<\/aside>/, navigation)
    .replace('</style>', `${styles}</style>`)
    .replace('Compare the three layouts using the same artwork, type and colors. Narrow view shows how each layout stacks on a phone.', 'The Music screen stays Wall + inspector. These options change the application hierarchy, not the music workflow. TV and Movies are not implemented.')
    .replace('</body>', `<script nonce="__NONCE__">${script}</script></body>`);
}
