import { data } from "./test/data.js";

export default {
  async fetch(request, env, ctx) {
    const response = new Response("Hello World " + data(), { status: 200 });
    response.headers.set("Canary", "true");
    response.headers.set("Access-Control-Allow-Origin", "*");
    response.headers.set("Access-Control-Allow-Methods", "*");
    response.headers.set("Access-Control-Allow-Headers", "*");
    return response;
  },
};
