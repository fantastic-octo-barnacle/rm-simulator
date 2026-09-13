# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
FROM python:3.13-alpine
WORKDIR /app
COPY server.py .
USER 65534:65534
EXPOSE 7791
HEALTHCHECK --interval=30s --timeout=5s CMD python -c "import urllib.request; urllib.request.urlopen('http://127.0.0.1:7791/health', timeout=3)"
CMD ["python", "-B", "server.py"]
