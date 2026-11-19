const http = require('http');
const fs = require('fs');
const path = require('path');

const repo = path.resolve(__dirname, '..', '..');
const sdk = process.env.CUBISM_SDK ?? path.join(repo, '.refs', 'cubism-web-sdk', 'CubismSdkForWeb-5-r.4');
const port = Number(process.argv[2] ?? 8731);

const routes = [
  ['/web/', __dirname],
  ['/sdk/', sdk],
  ['/res/', path.join(repo, '.refs', 'cubism-web-samples', 'Samples', 'Resources')],
  ['/rig/', path.join(repo, 'DeepSeek-fish-live2d')]
];

const types = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.moc3': 'application/octet-stream'
};

const resolve = url => {
  const file = decodeURIComponent(url.split('?')[0]);

  for (const [prefix, base] of routes) {
    if (file.startsWith(prefix)) {
      const target = path.join(base, file.slice(prefix.length));

      if (!path.resolve(target).startsWith(path.resolve(base))) {
        return null;
      }

      return target;
    }
  }

  return path.join(__dirname, 'blank.html');
};

http
  .createServer((request, response) => {
    const target = resolve(request.url);

    if (!target) {
      response.writeHead(403).end('forbidden');
      return;
    }

    fs.readFile(target, (error, body) => {
      if (error) {
        response.writeHead(404).end(`${request.url}: ${error.code}`);
        return;
      }

      response.writeHead(200, {
        'content-type': types[path.extname(target)] ?? 'application/octet-stream',
        'cache-control': 'no-store'
      });
      response.end(body);
    });
  })
  .listen(port, '127.0.0.1', () => console.log(`listening on http://127.0.0.1:${port}, sdk ${sdk}`));
