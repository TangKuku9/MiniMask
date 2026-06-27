const http = require('http')
const s = http.createServer((req, res) => {
  res.writeHead(200, { 'Content-Type': 'text/plain' })
  res.end('hello from tunneled service\n')
})
s.listen(9000, '127.0.0.1', () => console.log('test server on 127.0.0.1:9000'))
