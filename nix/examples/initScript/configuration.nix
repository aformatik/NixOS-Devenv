{

  codchi.secrets.env.TEST.description = ''
    This is a example secret.
  '';

  codchi.initScript = ''
    echo $CODCHI_TEST


    mkdir test_dir


    pwd
  '';

}
