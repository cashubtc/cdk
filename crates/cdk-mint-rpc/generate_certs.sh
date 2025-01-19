# Generate private key for Certificate Authority (CA)
openssl genrsa -out ca.key 4096

# Generate CA certificate
openssl req -new -x509 -days 365 -key ca.key -out ca.pem -subj "/C=US/ST=State/L=City/O=Organization/OU=Unit/CN=MyCA"

# Generate private key for Server
openssl genrsa -out server.key 4096

# Generate Certificate Signing Request (CSR) for Server
openssl req -new -key server.key -out server.csr -subj "/C=US/ST=State/L=City/O=Organization/OU=Unit/CN=localhost"

# Generate Server certificate
openssl x509 -req -days 365 -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out server.pem -extfile <(printf "subjectAltName=DNS:localhost,DNS:my-server,IP:127.0.0.1")

# Generate private key for Client
openssl genrsa -out client.key 4096

# Generate CSR for Client
openssl req -new -key client.key -out client.csr -subj "/C=US/ST=State/L=City/O=Organization/OU=Unit/CN=client"

# Generate Client certificate
openssl x509 -req -days 365 -in client.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out client.pem

# Verify the certificates
echo "Verifying Server Certificate:"
openssl verify -CAfile ca.pem server.pem

echo "Verifying Client Certificate:"
openssl verify -CAfile ca.pem client.pem

# Clean up CSR files (optional)
rm server.csr client.csr

# Display certificate information
echo "Server Certificate Info:"
openssl x509 -in server.pem -text -noout | grep "Subject:\|Issuer:\|DNS:\|IP Address:"

echo "Client Certificate Info:"
openssl x509 -in client.pem -text -noout | grep "Subject:\|Issuer:"

# Final files you'll need:
# - ca.pem (Certificate Authority certificate)
# - server.key (Server private key)
# - server.pem (Server certificate)
# - client.key (Client private key)
# - client.pem (Client certificate)
