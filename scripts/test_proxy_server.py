import socket
import threading
import select
import sys

PORT = 8080
HOST = "127.0.0.1"

def handle_client(client_socket, client_addr):
    try:
        request = client_socket.recv(4096)
        if not request:
            return
        
        request_line = request.split(b"\r\n")[0].decode("latin1", errors="ignore")
        parts = request_line.split(" ")
        if len(parts) < 2:
            return
        
        method = parts[0].upper()
        target = parts[1]

        if method == "CONNECT":
            if ":" in target:
                remote_host, remote_port = target.split(":", 1)
                remote_port = int(remote_port)
            else:
                remote_host, remote_port = target, 443

            print(f"[PROXY] Conexao HTTPS tunelada para: {remote_host}:{remote_port} (de {client_addr})")
            
            try:
                remote_socket = socket.create_connection((remote_host, remote_port), timeout=10)
            except Exception as e:
                print(f"[PROXY] Falha ao conectar no destino {remote_host}:{remote_port} - {e}")
                client_socket.sendall(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
                return

            client_socket.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")

            sockets = [client_socket, remote_socket]
            while True:
                r, _, _ = select.select(sockets, [], [], 15)
                if not r:
                    break
                for s in r:
                    other = remote_socket if s is client_socket else client_socket
                    data = s.recv(16384)
                    if not data:
                        return
                    other.sendall(data)
        else:
            print(f"[PROXY] Requisicao HTTP {method}: {target}")
            client_socket.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")

    except Exception:
        pass
    finally:
        try:
            client_socket.close()
        except:
            pass

def main():
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        server.bind((HOST, PORT))
    except Exception as e:
        print(f"Erro ao abrir a porta {PORT}: {e}")
        sys.exit(1)

    server.listen(50)
    print("=" * 55)
    print(" Servidor Proxy HTTP de Teste Ativo!")
    print(f" Endereco: {HOST}")
    print(f" Porta:    {PORT}")
    print(f" No Litecord: Selecione 'Proxy HTTP / HTTPS', Host '127.0.0.1' e Porta '{PORT}'")
    print(" Para parar o proxy a qualquer momento, aperte Ctrl + C")
    print("=" * 55)

    while True:
        try:
            client, addr = server.accept()
            t = threading.Thread(target=handle_client, args=(client, addr), daemon=True)
            t.start()
        except KeyboardInterrupt:
            print("\n Proxy encerrado pelo usuario.")
            break
        except Exception:
            break

if __name__ == "__main__":
    main()
