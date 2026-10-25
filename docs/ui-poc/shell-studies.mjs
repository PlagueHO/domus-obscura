const icons = {
  music: '<path d="M9 18V5l11-2v13M9 8l11-2"/><ellipse cx="6" cy="18" rx="3" ry="2"/><ellipse cx="17" cy="16" rx="3" ry="2"/>',
  tv: '<rect x="3" y="6" width="18" height="14" rx="2"/><path d="m8 2 4 4 4-4"/>',
  movies: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M7 3v18M17 3v18M3 8h4m-4 8h4M17 8h4m-4 8h4"/>',
  search: '<circle cx="10" cy="10" r="6"/><path d="m15 15 6 6"/>',
  library: '<path d="M4 4v16M9 4v16M14 4v16m3-15 4 14"/>',
  queue: '<path d="M9 5h12M9 12h12M9 19h12"/><circle cx="4" cy="5" r="1"/><circle cx="4" cy="12" r="1"/><circle cx="4" cy="19" r="1"/>',
  bell: '<path d="M5 9a7 7 0 0 1 14 0v6l2 3H3l2-3ZM9 21h6"/>',
  chat: '<path d="M21 16a3 3 0 0 1-3 3H9l-6 3V6a3 3 0 0 1 3-3h12a3 3 0 0 1 3 3Z"/><path d="M7 8h10M7 13h6"/>',
  settings: '<path d="m9 3-1 3-3 1-2 5 2 5 3 1 1 3h6l1-3 3-1 2-5-2-5-3-1-1-3Z"/><circle cx="12" cy="12" r="3"/>',
  account: '<circle cx="12" cy="8" r="4"/><path d="M4 22v-3a8 8 0 0 1 16 0v3"/>',
  collapse: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18m7-13-3 4 3 4"/>',
  close: '<path d="m6 6 12 12M6 18 18 6"/>',
};
const icon = (name) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${icons[name]}</svg>`;
const coreButtons = () => `<button class="side-item selected" data-preview-message="Music is the active core." title="Music" aria-current="page">${icon('music')}<span class="side-label">Music</span></button>
  <button class="side-item" disabled title="TV - future core">${icon('tv')}<span class="side-label">TV <small>Future</small></span></button>
  <button class="side-item" disabled title="Movies - future core">${icon('movies')}<span class="side-label">Movies <small>Future</small></span></button>`;
const pageButtons = () => `<button class="side-item selected" data-page="music" title="Music search" aria-current="page">${icon('search')}<span class="side-label">Music</span></button>
  <button class="side-item" data-page="library" title="Music library">${icon('library')}<span class="side-label">Library</span></button>`;
const pageTabs = () => `<nav class="page-tabs" aria-label="Music pages"><button data-page="music" aria-current="page">Music</button><button data-page="library">Library</button></nav>`;
const coreTabs = () => `<nav class="core-tabs" aria-label="Core functions"><button aria-current="page" data-preview-message="Music is the active core.">${icon('music')}Music</button><button disabled>${icon('tv')}TV <small>Future</small></button><button disabled>${icon('movies')}Movies <small>Future</small></button></nav>`;

export function shellStudy(html) {
  const sidebar = `<aside class="sidebar study-sidebar" aria-label="Application sidebar">
    <button class="side-item sidebar-toggle" aria-label="Collapse sidebar" aria-expanded="true" title="Collapse sidebar">${icon('collapse')}<span class="side-label">Collapse sidebar</span></button>
    <div class="sidebar-core-list"><nav aria-label="Core functions">${coreButtons()}</nav></div>
    <div class="sidebar-page-list"><div class="side-context side-label">Music management</div><nav aria-label="Music pages">${pageButtons()}</nav></div>
    <div class="sidebar-group-list"><details open><summary>${icon('music')}<span class="side-label">Music</span></summary><nav aria-label="Music pages">${pageButtons()}</nav></details><nav aria-label="Future cores">${coreButtons().replace(/<button class="side-item selected"[\s\S]*?<\/button>/, '')}</nav></div>
    <div class="sidebar-picker"><label class="side-label" for="shell-core">Core function</label><select id="shell-core" aria-label="Core function"><option>Music management</option><option disabled>TV - future</option><option disabled>Movies - future</option></select><button class="side-item picker-icon selected" title="Music management">${icon('music')}<span class="side-label">Music</span></button></div>
    <div class="sidebar-bottom"><button class="side-item" data-panel="settings" title="App settings">${icon('settings')}<span class="side-label">App settings</span></button><button class="side-item" data-panel="account" title="Account">${icon('account')}<span class="side-label">Account</span></button></div>
  </aside>`;
  const appHeader = `<header class="app-toolbar"><a class="app-title" href="#music">DOMUS OBSCURA</a>
    <nav class="global-tools" aria-label="Global tools"><button data-panel="queue" aria-label="Open global queue, 3 sample tasks" title="Global queue">${icon('queue')}<span class="tool-text">Queue</span><span class="count">3</span></button><button data-panel="notifications" aria-label="Open notifications, 1 sample notice" title="Notifications">${icon('bell')}<span class="notice-dot"></span></button><button data-panel="chat" aria-label="Open chat preview" title="Chat preview">${icon('chat')}</button></nav></header>
    `;
  const header = `<div class="top-core-tabs">${coreTabs()}</div><div class="top-page-tabs">${pageTabs()}</div><div class="under-tabs">${pageTabs()}</div><nav class="work-tabs" aria-label="Open work examples"><button data-page="music" aria-current="page">${icon('search')}Music search</button><button data-page="library">${icon('library')}Music library</button></nav>`;
  const drawer = `<aside class="utility-panel" id="utility-panel" aria-label="Global tool" hidden><div class="panel-heading"><h2 id="panel-title"></h2><button class="close-panel" aria-label="Close panel" title="Close panel">${icon('close')}</button></div><div id="panel-body"></div></aside>`;
  const styles = `
.shell[data-study]{position:relative;grid-template-columns:190px minmax(0,1fr);min-height:850px}.study-sidebar{padding:18px 10px;gap:0;min-width:0;flex-direction:column;border-right:1px solid #3c3149}
.brand-row{min-height:75px;padding:3px 8px}.compact-brand{display:flex;align-items:start;font-family:Poster,sans-serif;font-size:31px;line-height:1;color:#d8bdff}.compact-brand .side-label{font-size:21px;line-height:1.05;padding:1px 0 0 2px}.side-item{display:flex;align-items:center;gap:12px;width:100%;min-height:46px;background:none;border:0;text-align:left;padding:11px 12px;color:#d0c3de;margin:3px 0;white-space:nowrap}.side-item svg,.global-tools svg,.core-tabs svg,.work-tabs svg,.panel-heading svg,.sidebar-group-list summary svg{width:21px;height:21px;flex:none}.side-item.selected{background:#30233f;color:#e5cdff}.side-item:disabled{color:#baaccb;opacity:1}.side-item:disabled:hover{background:none}.side-item small{display:block;font-size:10px;color:#bfb0cf}.sidebar-toggle{margin-bottom:27px;color:#c7b7d9}.sidebar-bottom{margin-top:auto;padding-top:55px}.sidebar-core-list,.sidebar-page-list,.sidebar-group-list,.sidebar-picker{display:none}.side-context{font-size:12px;color:#c9b4df;padding:8px 12px 13px}.sidebar-group-list summary{display:flex;gap:11px;align-items:center;padding:10px;min-height:44px;cursor:pointer;color:#e2ccfc}.sidebar-group-list details nav{padding-left:13px}.sidebar-picker label{font-size:12px;color:#c9b4df}.sidebar-picker select{width:100%;margin-top:8px;background:#24202f;border:1px solid #8a739f;color:#f1e6fc;padding:8px 5px;font-size:12px}.picker-icon{display:none}
.app-body{min-width:0;position:relative}.app-toolbar{min-height:83px;border-bottom:1px solid #3b3047;padding:15px 26px;display:flex;align-items:center;gap:18px;background:#14111c}.toolbar-context{display:grid;gap:4px}.toolbar-context>span{font-family:Poster,sans-serif;color:#cbb5e5;font-size:20px;letter-spacing:.025em}.toolbar-context>strong{font-size:13px;font-weight:500;color:#e9def4}.global-tools{margin-left:auto;display:flex;align-items:center;gap:6px}.global-tools button{position:relative;display:flex;align-items:center;justify-content:center;gap:8px;min-width:44px;background:none;border-color:transparent;padding:10px;color:#ddcdec}.global-tools button:hover,.global-tools button[aria-expanded=true]{background:#34243f;border-color:#85659f}.count{font-size:11px;border:1px solid #9472b1;border-radius:3px;padding:0 5px;color:#eadbff}.notice-dot{width:6px;height:6px;background:#c4a1ff;border-radius:50%;position:absolute;right:8px;top:8px}.tool-text{font-size:13px}
.page-tabs,.core-tabs,.work-tabs{display:flex;gap:22px;align-items:center}.page-tabs button,.core-tabs button,.work-tabs button{display:flex;align-items:center;gap:9px;background:none;border:0;border-radius:0;color:#cdbedc;padding:12px 3px;min-height:48px;white-space:nowrap}.page-tabs button[aria-current],.core-tabs button[aria-current],.work-tabs button[aria-current]{box-shadow:inset 0 -2px #c39bff;color:#f0e3ff}.core-tabs{gap:20px}.core-tabs small{font-size:10px;color:#c4b3d4}.core-tabs button:disabled{color:#b9a8cb;opacity:1}.under-tabs{padding:0 28px;border-bottom:1px solid #352b41}.top-core-tabs,.top-page-tabs,.work-tabs{display:none}.content{padding-top:27px}.content .page-title h1{font-size:40px}.utility-panel{position:absolute;right:0;top:0;bottom:0;width:362px;max-width:100%;z-index:4;background:#211928;border-left:1px solid #9477af;padding:23px 22px;box-shadow:-12px 0 32px #0006;overflow:auto;animation:panel-in 180ms ease-out}.utility-panel[hidden]{display:none}.panel-heading{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-bottom:29px}.panel-heading h2{font-size:30px}.close-panel{background:none;border-color:#6e557e;padding:10px;display:grid;place-items:center}.panel-note{color:#d1bddf;font-size:13px;line-height:1.6}.queue-scope{display:flex;gap:8px;flex-wrap:wrap;margin:24px 0}.queue-scope span{padding:4px 8px;border:1px solid #8b739d;font-size:12px;border-radius:3px}.queue-task{border-top:1px solid #665072;padding:18px 0}.queue-task strong{display:block;font-size:14px}.queue-task p{font-size:13px;margin:6px 0 0;color:#d3bedf}.queue-task small{display:block;color:#d2b4fa;margin-top:8px;font-size:12px}.chat-question{padding:14px;background:#35263f;font-size:14px;margin-top:27px;line-height:1.6}.chat-compose{display:grid;gap:12px;margin-top:28px}.chat-compose label{font-size:13px;color:#dfcee9}.chat-compose textarea{resize:vertical;min-height:100px;width:100%;background:#17111e;border:1px solid #8b749d;color:#ecdcfa;padding:12px;font:inherit}.chat-compose button:disabled{opacity:1;background:#45314e;color:#d5bedf;border-color:#795d8b;cursor:not-allowed}.panel-link{width:100%;margin-top:22px;font-size:13px}
.shell[data-study=core-rail] .sidebar-core-list,.shell[data-study=focus] .sidebar-core-list{display:block}
.shell[data-study=core-tabs] .sidebar-page-list{display:block}.shell[data-study=core-tabs] .top-core-tabs{display:block}.shell[data-study=core-tabs] .toolbar-context,.shell[data-study=core-tabs] .under-tabs{display:none}
.shell[data-study=work-tabs] .sidebar-group-list{display:block}.shell[data-study=work-tabs] .under-tabs{display:none}.shell[data-study=work-tabs] .work-tabs{display:flex;padding:0 28px;gap:9px;border-bottom:1px solid #3c3048}.work-tabs button{padding:12px 15px;background:#1a1423}.work-tabs button[aria-current]{background:#30213f}
.shell[data-study=switcher] .sidebar-picker{display:block}
.shell[data-study=focus] .top-page-tabs{display:block}.shell[data-study=focus] .toolbar-context,.shell[data-study=focus] .under-tabs{display:none}.shell[data-study=focus] .app-toolbar{min-height:70px}.shell[data-study=focus] .utility-panel{top:70px}
.shell.collapsed{grid-template-columns:68px minmax(0,1fr)}.collapsed .study-sidebar{padding-left:7px;padding-right:7px}.collapsed .side-label,.collapsed .sidebar-picker select{display:none}.collapsed .compact-brand{font-size:32px}.collapsed .brand-row{padding:7px 12px}.collapsed .side-item{justify-content:center;padding:11px}.collapsed .sidebar-group-list details nav{padding-left:0}.collapsed .sidebar-group-list summary{justify-content:center}.collapsed .sidebar-group-list summary svg{width:23px}.collapsed .sidebar-picker .picker-icon{display:flex}
.sample-library{padding:20px 0}.sample-library p{font-size:14px;color:#ccb9dc;max-width:60ch}.sample-library .tiles{margin-top:24px}.sample-library h2{font-size:31px}
@keyframes panel-in{from{transform:translateX(25px);opacity:.5}to{transform:translateX(0);opacity:1}}
@container(min-width:1250px){.shell[data-study=focus].panel-open .app-body{padding-right:350px}.shell[data-study=focus] .utility-panel{width:350px;position:absolute;top:70px}.shell[data-study=focus].panel-open .workspace{display:block}.shell[data-study=focus].panel-open .inspector{border-left:0;border-top:1px solid #5c416e;padding:24px 0;margin-top:28px}}
@container(max-width:1000px){.shell[data-study]{grid-template-columns:164px minmax(0,1fr)}.shell.collapsed{grid-template-columns:68px minmax(0,1fr)}.app-toolbar{padding:12px 16px}.core-tabs{gap:12px}.core-tabs button{font-size:13px;gap:6px}.core-tabs small{display:none}.tool-text{display:none}}
@container(max-width:700px){.shell[data-study]{grid-template-columns:60px minmax(0,1fr)}.shell:not(.collapsed){grid-template-columns:158px minmax(0,1fr)}.study-sidebar{border-bottom:0;padding:16px 5px;align-items:stretch;flex-wrap:nowrap}.sidebar-toggle{margin-bottom:20px}.sidebar-bottom{padding-top:40px}.brand-row{min-height:61px}.collapsed .brand-row{padding:6px 10px}.side-item{gap:8px;padding:9px;font-size:13px}.side-item svg{width:20px;height:20px}.app-toolbar{padding:12px 8px;gap:6px;flex-wrap:wrap;min-height:75px}.toolbar-context>span{font-size:18px}.toolbar-context>strong{font-size:11px}.global-tools{gap:0}.global-tools button{padding:8px;min-width:44px}.global-tools .count{position:absolute;right:0;top:0;background:#30213e;font-size:10px}.top-core-tabs{width:100%;order:2}.core-tabs{gap:14px;justify-content:flex-start;flex-wrap:wrap}.core-tabs button{gap:5px;font-size:12px;padding:7px 0;min-height:44px}.core-tabs svg{width:17px;height:17px}.shell[data-study=core-tabs] .global-tools{margin-left:auto}.under-tabs{padding:0 15px}.page-tabs{gap:24px}.content{padding:22px 14px}.content .page-title h1{font-size:35px}.workspace{display:block}.inspector{border-left:0;border-top:1px solid #634b74;padding:24px 0 0;margin-top:25px}.album{flex-basis:132px}.work-tabs{padding:0 8px!important;gap:4px!important;flex-wrap:wrap}.work-tabs button{font-size:12px;padding:10px 7px}.work-tabs svg{width:17px;height:17px}.shell[data-study=focus] .top-page-tabs{order:2;width:100%}.shell[data-study=focus] .utility-panel{top:0}.utility-panel{width:100%;padding:20px 16px}.shell:not(.collapsed) .toolbar-context{display:none}.shell:not(.collapsed) .content{padding-left:10px;padding-right:10px}.shell:not(.collapsed) .search-field input{font-size:12px}.shell:not(.collapsed) .core-tabs{gap:9px}.shell:not(.collapsed) .core-tabs svg{display:none}}
@container(max-width:700px){.selection{flex-wrap:wrap}.selection>div{min-width:0;overflow-wrap:anywhere}.search-field{min-width:0}}
.shell[data-study]{grid-template-rows:auto minmax(850px,1fr)}
.shell[data-study]>.app-toolbar{grid-column:1/-1;grid-row:1;min-height:76px;padding:14px 24px;flex-wrap:nowrap}
.app-title{font-family:Poster,sans-serif;font-size:29px;line-height:1.1;letter-spacing:.025em;color:#e5d4ff;display:flex;align-items:center;white-space:nowrap}
.study-sidebar,.app-body{grid-row:2}
.shell[data-study] .top-core-tabs,.shell[data-study] .top-page-tabs{padding:0 28px;border-bottom:1px solid #3b3047}
.shell[data-study=focus] .utility-panel{top:0}
@container(max-width:700px){.shell[data-study]>.app-toolbar{padding:12px 14px;min-height:70px;gap:8px}.app-title{font-size:24px}.shell[data-study] .top-core-tabs,.shell[data-study] .top-page-tabs{padding:0 14px}.app-toolbar .global-tools{margin-left:auto}.app-toolbar .tool-text{display:none}}
@media(prefers-reduced-motion:reduce){.utility-panel{animation:none}}
`;
  const script = `
const appShell=document.querySelector('.shell');
const preview=document.querySelector('#preview');
const panels=document.querySelector('#utility-panel');
const originalContent=document.querySelector('.content').innerHTML;
let panelOpener=null;
const variants={
 'core-rail':['Core sidebar + page tabs','Cores live in the sidebar. Music and Library are tabs. Recommended: clear hierarchy without a second full sidebar.'],
 'core-tabs':['Core tabs + page sidebar','Music, TV and Movies sit across the top. The sidebar holds the active core pages. Best when switching cores often.'],
 'work-tabs':['Grouped sidebar + work tabs','Core groups hold their pages. Open work has tabs across the top. This could keep a search and library view open together; saved tab state is not implemented.'],
 'switcher':['Core switcher + page tabs','A core selector lives in the sidebar. Page tabs stay above the artwork. Quietest layout as the number of cores grows.'],
 'focus':['Compact rail + inline tabs','A compact core rail and inline page tabs leave more room for artwork. At wide widths, the right panel pushes content aside instead of covering it.']
};
function closePanel(){
 panels.hidden=true;appShell.classList.remove('panel-open');
 document.querySelectorAll('[data-panel]').forEach(b=>b.setAttribute('aria-expanded','false'));
 panelOpener?.focus({preventScroll:true});
}
function setCollapsed(collapsed){
 appShell.classList.toggle('collapsed',collapsed);
 const b=document.querySelector('.sidebar-toggle');b.setAttribute('aria-expanded',String(!collapsed));
 b.setAttribute('aria-label',collapsed?'Expand sidebar':'Collapse sidebar');b.title=collapsed?'Expand sidebar':'Collapse sidebar';
 b.querySelector('.side-label').textContent=collapsed?'Expand sidebar':'Collapse sidebar';
}
function setStudy(mode){
 appShell.dataset.study=mode;
 document.querySelectorAll('[data-shell-study]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.shellStudy===mode)));
 document.querySelector('#description').textContent=variants[mode][1];
 closePanel();setCollapsed(mode==='focus'||preview.classList.contains('narrow')||preview.clientWidth<700);
}
function showPage(page){
 document.querySelectorAll('[data-page]').forEach(b=>{if(b.dataset.page===page)b.setAttribute('aria-current','page');else b.removeAttribute('aria-current');});
 if(page==='music'){document.querySelector('.content').innerHTML=originalContent;document.querySelector('#search').addEventListener('input',render);document.querySelector('#format').addEventListener('change',e=>{document.querySelector('#feedback').textContent='Sample format preference: '+e.target.value+'. No media was checked.';});render();}
 else document.querySelector('.content').innerHTML='<div class="page-title"><h1>MUSIC LIBRARY</h1><p>Generated layout example. No indexed library is connected.</p></div><section class="sample-library"><h2>YOUR ALBUMS</h2><p>This shows the Library page position only. These sample covers do not represent files or completed jobs.</p><div class="tiles">'+albums.slice(0,3).map(a=>tile(a).replace('data-album=','data-sample-album=')).join('')+'</div></section>';
}
const panelContent={
 queue:['GLOBAL QUEUE','<p class="panel-note">Illustrative tasks across all cores. One scheduler would apply shared concurrency limits. Each core would keep its own rules and approvals.</p><div class="queue-scope"><span>All cores</span><span>Music</span><span>TV / future</span><span>Movies / future</span></div><div class="queue-task"><strong>Glass Satellites</strong><p>Music / verifying output</p><small>Sample active task</small></div><div class="queue-task"><strong>Example TV season</strong><p>TV / waiting for a shared slot</p><small>Future-core example only</small></div><div class="queue-task"><strong>Example movie</strong><p>Movies / waiting for a shared slot</p><small>Future-core example only</small></div><p class="panel-note">The actual concurrency limit is not decided here. Provider limits, approval waits and safe pause boundaries remain separate.</p>'],
 notifications:['NOTIFICATIONS','<p class="panel-note">Sample inbox notice. No live account or task is connected.</p><div class="queue-task"><strong>Artwork needs your review</strong><p>Music / Glass Satellites</p><small>Preview only / no approval action</small></div><p class="panel-note">Notifications can link to a core task without changing its approval state.</p>'],
 chat:['CHAT','<p class="panel-note">Future chat surface. No model, agent, provider or MCP connection is active. Runtime AI remains outside the current V1 scope.</p><div class="chat-question">Which albums by Vanta Circuit are missing from my library?</div><p class="panel-note">Example prompt only. No request was sent.</p><div class="chat-compose"><label for="chat-draft">Try the input layout</label><textarea id="chat-draft" placeholder="Ask about music, TV or movies"></textarea><button disabled>Send unavailable in preview</button></div>'],
 settings:['APP SETTINGS','<p class="panel-note">Shared application settings would live here. Provider and media rules would remain scoped to their core.</p><div class="queue-task"><strong>Global work limits</strong><p>Concurrent transfers and disk-heavy work would be configured here. This study changes no settings.</p></div>'],
 account:['ACCOUNT','<p class="panel-note">Account control position only. No live identity or session data is loaded.</p><div class="queue-task"><strong>Personal account</strong><p>Identity and sign-out controls would live here.</p></div>']
};
document.addEventListener('click',e=>{
 const variant=e.target.closest('[data-shell-study]');if(variant)setStudy(variant.dataset.shellStudy);
 if(e.target.closest('.sidebar-toggle'))setCollapsed(!appShell.classList.contains('collapsed'));
 if(e.target.closest('.narrow-toggle')&&preview.classList.contains('narrow'))setCollapsed(true);
 const page=e.target.closest('[data-page]');if(page)showPage(page.dataset.page);
 const utility=e.target.closest('[data-panel]');
 if(utility){
  if(!panels.hidden&&panels.dataset.panel===utility.dataset.panel){closePanel();return;}
  panelOpener=utility;const data=panelContent[utility.dataset.panel];
  document.querySelectorAll('[data-panel]').forEach(b=>b.setAttribute('aria-expanded',String(b===utility)));
  panels.dataset.panel=utility.dataset.panel;document.querySelector('#panel-title').textContent=data[0];document.querySelector('#panel-body').innerHTML=data[1];
  panels.hidden=false;appShell.classList.add('panel-open');panels.querySelector('.close-panel').focus({preventScroll:true});
 }
 if(e.target.closest('.close-panel'))closePanel();
 const message=e.target.closest('[data-preview-message]');if(message)document.querySelector('#feedback').textContent=message.dataset.previewMessage+' No application action was taken.';
});
document.addEventListener('keydown',e=>{if(e.key==='Escape'&&!panels.hidden)closePanel();});
document.querySelectorAll('[data-panel]').forEach(b=>{b.setAttribute('aria-controls','utility-panel');b.setAttribute('aria-expanded','false');});
setStudy('core-rail');
document.querySelector('#feedback').textContent='Generated samples only. Queue is global in this study. Chat, TV and Movies have no runtime implementation.';
`;
  return html
    .replace('Domus Obscura - Cover wall layout studies', 'Domus Obscura - five application shell studies')
    .replace('Domus Obscura / design preview', 'Application shell / design preview')
    .replace('Generated sample albums. No provider, approval or download access.', 'Five shell studies. Sample data only. No downloads, approvals or AI calls.')
    .replace(/<div class="study-controls"[\s\S]*?<\/div>/, `<div class="study-controls" aria-label="Application shell variations">
      <button data-shell-study="core-rail" aria-pressed="true">1. Core sidebar</button>
      <button data-shell-study="core-tabs" aria-pressed="false">2. Core tabs</button>
      <button data-shell-study="work-tabs" aria-pressed="false">3. Work tabs</button>
      <button data-shell-study="switcher" aria-pressed="false">4. Core switcher</button>
      <button data-shell-study="focus" aria-pressed="false">5. Focus layout</button>
      <button class="narrow-toggle" aria-pressed="false">Narrow view</button></div>`)
    .replace(/<aside class="sidebar">[\s\S]*?<\/aside>/, `${appHeader}${sidebar}<div class="app-body">${header}`)
    .replace(/<\/main>\s*<\/div>/, `</main>${drawer}</div>\n  </div>`)
    .replace(/document\.querySelector\('#description'\)\.textContent\s*=\s*descriptions\[layout\];/, '')
    .replace('</style>', `${styles}</style>`)
    .replace('Compare the three layouts using the same artwork, type and colors. Narrow view shows how each layout stacks on a phone.', 'Try all five options. Collapse the sidebar. Open Queue, notifications and Chat at the top right. Account and App settings stay at the bottom left.')
    .replace('</body>', `<script nonce="__NONCE__">${script}</script></body>`);
}
