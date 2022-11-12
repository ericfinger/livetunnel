# livetunnel - Tunnel your local files to your own Webserver

Inspired by [this Blogpost](https://igauravsehrawat.com/build-your-own-ngrok-4-easy-steps/), I wanted to write a CLI Program to automatically tunnel HTTP(S)-Requests from a webserver you own to your local filesystem.

## Features

- Opens an SSH Tunnel to your server and forwards the necessary ports
  - Supports custom connect-commands (for port-knocking etc)
- Acts as a frontend to the excellent [miniserve](https://github.com/svenstaro/miniserve) to (optionally) serve local files
    - Can serve files and websites
    - Allows to protect content with username/password
    - Allows uploads

-------------------

## Example Nginx Config

```nginx
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}

server {
    server_name [YOUR SERVER URL];

    location / {
        proxy_pass http://localhost:[YOUR PORT];
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header Host $http_host;
        proxy_set_header X-NginX-Proxy true;

        # Enables WS support
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
    }

    location /secure/ {
        auth_basic "Restricted Access!";
        auth_basic_user_file /etc/nginx/conf.d/.htpasswd;

        rewrite /secure/(.*) /$1  break;
        proxy_pass http://localhost:[YOUR PORT];
        proxy_redirect off;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header Host $http_host;
        proxy_set_header X-NginX-Proxy true;

        # Enables WS support
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
    }

    listen 443 ssl; # managed by Certbot
    ssl_certificate /etc/letsencrypt/live/[YOUR SERVER URL]/fullchain.pem; # managed by Certbot
    ssl_certificate_key /etc/letsencrypt/live/[YOUR SERVER URL]/privkey.pem; # managed by Certbot
    include /etc/letsencrypt/options-ssl-nginx.conf; # managed by Certbot
    ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem; # managed by Certbot

}

server {
    if ($host = [YOUR SERVER URL]) {
        return 301 https://$host$request_uri;
    } # managed by Certbot

    server_name [YOUR SERVER URL];
    listen 80;
    return 404; # managed by Certbot
}

```