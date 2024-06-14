{

  codchi.secrets.env.TEST.description = ''
    This is a example secret.
  '';


  systemd.services.my-secret-service = {
    description = "My Service which reads codchi's secrets";
    wantedBy = [ "multi-user.target" ]; # Start this service in multi-user mode

    script = ''
      source /etc/codchi-env
      echo "Got secret CODCHI_TEST=$CODCHI_TEST from codchi"
    '';

  };

}
