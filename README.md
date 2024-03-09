# iframe traffic counter

***Binds to localhost:32069 by default!***

Example usage...

```html
<iframe
    id="traffic-counter"
    width="140"
    height="40"
    src="http://localhost:32069/"
></iframe>
```

... obviously, replace the localhost with something else when actually using it.


## how does it work

it uses the HTTP "referer" header value to get the server name. make sure whatever you're using to proxy the program proxys that value correctly.

*you'll either need to know a server that hosts this program or host it yourself.*
