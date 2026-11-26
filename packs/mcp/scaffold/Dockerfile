# The hosted server: Streamable HTTP on $PORT, each request with its own API key, or with
# --oauth --store /data/grants.db, its own OAuth server.
FROM node:22-slim AS build
WORKDIR /app
COPY package.json package-lock.json* ./
RUN npm install --no-audit --no-fund
COPY tsconfig.json ./
COPY src ./src
RUN npm run build && npm prune --omit=dev

FROM node:22-slim
WORKDIR /app
ENV NODE_ENV=production PORT=8080
COPY --from=build /app/node_modules ./node_modules
COPY --from=build /app/dist ./dist
COPY package.json ./
RUN mkdir /data && chown node /data
USER node
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["node", "dist/main.js"]
