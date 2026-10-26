import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import { navigationStudy } from './navigation.mjs';
import { shellStudy } from './shell-studies.mjs';

const nonce = randomBytes(18).toString('base64');
const base = new URL('./', import.meta.url);
const files = new Map([
  ['/', ['index.html', 'text/html; charset=utf-8']],
  ['/navigation', ['index.html', 'text/html; charset=utf-8']],
  ['/shells', ['index.html', 'text/html; charset=utf-8']],
  ['/reference', ['reference.html', 'text/html; charset=utf-8']],
  ['/structure', ['structure.html', 'text/html; charset=utf-8']],
  ['/heading.ttf', ['BebasNeue-Regular.ttf', 'font/ttf']],
  ['/font-license.txt', ['OFL.txt', 'text/plain; charset=utf-8']],
]);
const server = createServer(async (request, response) => {
  response.setHeader('Cache-Control', 'no-store');
  response.setHeader('X-Content-Type-Options', 'nosniff');
  response.setHeader('Referrer-Policy', 'no-referrer');
  response.setHeader('Content-Security-Policy', `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'nonce-${nonce}'; font-src 'self'; img-src 'self' data:; connect-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`);
  if (request.headers.host !== `127.0.0.1:${server.address().port}`) {
    response.writeHead(403).end();
    return;
  }
  if (!['GET', 'HEAD'].includes(request.method)) {
    response.writeHead(405).end();
    return;
  }
  const item = files.get(request.url);
  if (!item) {
    response.writeHead(404).end();
    return;
  }
  try {
    let body = await readFile(new URL(item[0], base));
    if (item[0].endsWith('.html')) {
      let html=body.toString();
      if (request.url==='/navigation') html=navigationStudy(html);
      if (request.url==='/shells') html=shellStudy(html);
      body=Buffer.from(html.replaceAll('__NONCE__',nonce));
    }
    response.writeHead(200, { 'Content-Type': item[1], 'Content-Length': body.length });
    response.end(request.method === 'HEAD' ? undefined : body);
  } catch (error) {
    console.error(`Design asset could not be read: ${error.message}`);
    response.writeHead(500).end('Design asset could not be read.');
  }
});
server.listen(0, '127.0.0.1', () => console.log(`DESIGN URL: http://127.0.0.1:${server.address().port}/`));
