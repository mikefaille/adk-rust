        }
    }

    #[cfg(feature = "vertex")]
    fn vertex_client(&self, operation: &'static str) -> Result<&VertexClient, Error> {
        match &self.backend {
            #[cfg(feature = "vertex")]
            GeminiBackend::Vertex(vertex) => Ok(vertex),
            #[cfg(feature = "studio")]
            GeminiBackend::Rest(_) => GoogleCloudUnsupportedSnafu { operation }.fail(),
        }
    }

    /// Perform a GET request and deserialize the JSON response.
    #[tracing::instrument(skip(self), fields(request.type = "get", request.url = %url))]
    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: Url) -> Result<T, Error> {
        self.perform_request(|c| c.get(url), async |r| r.json().await.context(DecodeResponseSnafu))
            .await
    }

    /// Perform a POST request with JSON body and deserialize the JSON response.
    #[tracing::instrument(skip(self, body), fields(request.type = "post", request.url = %url))]
    async fn post_json<Req: serde::Serialize, Res: serde::de::DeserializeOwned>(
        &self,
        url: Url,
        body: &Req,
    ) -> Result<Res, Error> {
        self.perform_request(
            |c| c.post(url).json(body),
            async |r| r.json().await.context(DecodeResponseSnafu),
        )
        .await
    }

    /// Generate content
    #[instrument(skip_all, fields(
        model,
        messages.parts.count = request.contents.len(),
        tools.present = request.tools.is_some(),
        system.instruction.present = request.system_instruction.is_some(),
        cached.content.present = request.cached_content.is_some(),
    ), ret(level = Level::TRACE), err)]
