require "option_parser"

def toggle_rw(enable : Bool)
  if enable
    puts "Włączanie trybu zapisu (OverlayFS)..."
    # Tworzymy tymczasowe miejsce na zmiany w RAM (tmpfs)
    run_command("mount", ["-t", "tmpfs", "tmpfs", "/run/hammer_overlay_work"])
    Dir.mkdir_p("/run/hammer_overlay_work/upper")
    Dir.mkdir_p("/run/hammer_overlay_work/work")
    
    # Nakładamy OverlayFS na /etc (najczęstsze miejsce zmian)
    args = ["-t", "overlay", "overlay", "-o", 
            "lowerdir=/etc,upperdir=/run/hammer_overlay_work/upper,workdir=/run/hammer_overlay_work/work", "/etc"]
    output = run_command("mount", args)
    
    if output[:success]
      puts "System (/etc) jest teraz zapisywalny. Zmiany znikną po restarcie!"
    end
  else
    puts "Przywracanie trybu Read-Only..."
    run_command("umount", ["/etc"])
    puts "Zmiany zostały odrzucone."
  end
end

# Prosta obsługa argumentów
command = ARGV[0]?
case command
when "unlock" then toggle_rw(true)
when "lock"   then toggle_rw(false)
else puts "Użycie: hammer-read [unlock|lock]"
end
